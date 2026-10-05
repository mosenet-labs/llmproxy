//! Responses 文本立即发送，必需的 done 与终态快照使用有上限的类型化副本。
//! 参考：https://developers.openai.com/api/reference/resources/responses/streaming-events
use std::collections::BTreeMap;

use super::{Context, unsupported};
use crate::{
    adapter::{Error, Result, response::encode_responses_usage},
    ir::{
        response::{FinishReason, Status},
        stream::{Event, Head, Key},
    },
    protocol::{
        OptionalNullable as O,
        responses::{
            function::{Call, CallType},
            request::message::{
                AssistantRole, MessageType, OutputMessage, OutputPart, Status as ItemStatus,
            },
            response::{
                Response,
                body::{IncompleteDetails, OutputItem, ResponseError},
                event::{self as wire, KnownEvent as E},
            },
        },
        stream::Event as Raw,
    },
};
use serde_json::{Map, Value};

/// 一个 IR 内容块对应一个目标输出项，避免复用来源的稀疏输出索引。
struct Entry {
    index: u64,
    item: OutputItem,
}

/// 仅 Responses 保存已发送正文，用于协议必需的终态完整 output。
#[derive(Default)]
pub(super) struct Encoder {
    parts: BTreeMap<Key, Entry>,
    next: u64,
    sequence: u64,
    bytes: usize,
    reason: Option<FinishReason>,
}

impl Encoder {
    /// 返回仍保留在快照中的字符串及引用叶子长度。
    pub(super) fn buffered_bytes(&self) -> usize {
        self.bytes
    }

    /// 单条 IR 事件构造目标增量，不借用任何来源整包字段。
    pub(super) fn encode(&mut self, event: &Event, context: &mut Context<'_>) -> Result<()> {
        if self.bytes > context.remaining {
            return Err(unsupported(
                "stream.buffer",
                "参数与 Responses 快照合计超过缓冲上限",
            ));
        }
        match event {
            Event::Start(_) => {
                let response = self.response("in_progress", Vec::new(), context);
                let sequence_number = self.tick();
                emit(
                    context,
                    E::Created(wire::Lifecycle {
                        response: Box::new(response.clone()),
                        sequence_number,
                        extra: Default::default(),
                    }),
                );
                let sequence_number = self.tick();
                emit(
                    context,
                    E::InProgress(wire::Lifecycle {
                        response: Box::new(response),
                        sequence_number,
                        extra: Default::default(),
                    }),
                );
            }
            Event::PartStart {
                key,
                head: Head::Text | Head::Refusal | Head::Reasoning,
            } => {
                let id = context.item_id(*key);
                self.reserve(id.len(), context)?;
                let head = context.part(*key)?.head.clone();
                let item = if head == Head::Reasoning {
                    OutputItem::Other(Map::from_iter([
                        ("type".into(), Value::String("reasoning".into())),
                        ("id".into(), Value::String(id.clone())),
                        (
                            "summary".into(),
                            Value::Array(vec![Value::Object(Map::from_iter([
                                ("type".into(), Value::String("summary_text".into())),
                                ("text".into(), Value::String(String::new())),
                            ]))]),
                        ),
                    ]))
                } else {
                    let content = if head == Head::Refusal {
                        OutputPart::Refusal {
                            refusal: String::new(),
                            extra: Default::default(),
                        }
                    } else {
                        OutputPart::OutputText {
                            annotations: Vec::new(),
                            text: String::new(),
                            logprobs: O::Missing,
                            extra: Default::default(),
                        }
                    };
                    OutputItem::Message(OutputMessage {
                        id: id.clone(),
                        content: vec![content],
                        role: AssistantRole::Assistant,
                        status: ItemStatus::InProgress,
                        r#type: MessageType::Message,
                        phase: O::Missing,
                        extra: Default::default(),
                    })
                };
                let index = self.next;
                self.next += 1;
                let sequence_number = self.tick();
                let mut initial = item.clone();
                if let OutputItem::Message(message) = &mut initial {
                    message.content.clear();
                }
                emit(
                    context,
                    E::OutputItemAdded(wire::ItemEvent {
                        item: initial,
                        output_index: index,
                        sequence_number,
                        extra: Default::default(),
                    }),
                );
                if head == Head::Reasoning {
                    let sequence_number = self.tick();
                    emit(
                        context,
                        E::ReasoningSummaryPartAdded(wire::SummaryPart {
                            item_id: id,
                            output_index: index,
                            summary_index: 0,
                            part: wire::ReasoningPart::SummaryText {
                                text: String::new(),
                                extra: Default::default(),
                            },
                            sequence_number,
                            extra: Default::default(),
                        }),
                    );
                } else if let OutputItem::Message(message) = &item {
                    let sequence_number = self.tick();
                    emit(
                        context,
                        E::ContentPartAdded(wire::ContentEvent {
                            content_index: 0,
                            item_id: id,
                            output_index: index,
                            part: wire::ContentPart::Output(message.content[0].clone()),
                            sequence_number,
                            extra: Default::default(),
                        }),
                    );
                }
                self.parts.insert(*key, Entry { index, item });
            }
            Event::TextDelta { key, text } => {
                self.reserve(text.len(), context)?;
                let sequence_number = self.tick();
                let entry = self.entry_mut(*key)?;
                let id = item_id(&entry.item)?;
                let index = entry.index;
                let head = &context.part(*key)?.head;
                text_mut(&mut entry.item)?.push_str(text);
                if *head == Head::Reasoning {
                    emit(
                        context,
                        E::ReasoningSummaryTextDelta(wire::SummaryDelta {
                            delta: text.clone(),
                            item_id: id,
                            output_index: index,
                            summary_index: 0,
                            sequence_number,
                            obfuscation: O::Missing,
                            extra: Default::default(),
                        }),
                    );
                } else {
                    let delta = wire::ContentDelta {
                        content_index: 0,
                        delta: text.clone(),
                        item_id: id,
                        logprobs: O::Missing,
                        output_index: index,
                        sequence_number,
                        obfuscation: O::Missing,
                        extra: Default::default(),
                    };
                    emit(
                        context,
                        if *head == Head::Refusal {
                            E::RefusalDelta(delta)
                        } else {
                            E::OutputTextDelta(delta)
                        },
                    );
                }
            }
            Event::PartEnd(key) => {
                if let Head::Tool(head) = context.part(*key)?.head.clone() {
                    let arguments = context.part(*key)?.arguments.clone();
                    let id = context.item_id(*key);
                    let call_id = context.call_id(*key, &head);
                    let name = head.name.as_deref().unwrap_or_default();
                    self.reserve(
                        id.len() + call_id.len() + name.len() + arguments.len(),
                        context,
                    )?;
                    let index = self.next;
                    self.next += 1;
                    let item = if head.text_input {
                        OutputItem::Other(Map::from_iter([
                            ("type".into(), Value::String("custom_tool_call".into())),
                            ("id".into(), Value::String(id.clone())),
                            ("call_id".into(), Value::String(call_id)),
                            ("name".into(), Value::String(name.into())),
                            ("input".into(), Value::String(String::new())),
                        ]))
                    } else {
                        OutputItem::FunctionCall(Call {
                            arguments: String::new(),
                            call_id: O::Value(call_id),
                            name: name.into(),
                            r#type: CallType::FunctionCall,
                            id: O::Value(id.clone()),
                            status: O::Missing,
                            extra: Default::default(),
                        })
                    };
                    let sequence_number = self.tick();
                    emit(
                        context,
                        E::OutputItemAdded(wire::ItemEvent {
                            item: item.clone(),
                            output_index: index,
                            sequence_number,
                            extra: Default::default(),
                        }),
                    );
                    let sequence_number = self.tick();
                    let delta = wire::ItemDelta {
                        delta: arguments.clone(),
                        item_id: id.clone(),
                        output_index: index,
                        sequence_number,
                        obfuscation: O::Missing,
                        extra: Default::default(),
                    };
                    emit(
                        context,
                        if head.text_input {
                            E::CustomToolInputDelta(delta)
                        } else {
                            E::FunctionArgumentsDelta(delta)
                        },
                    );
                    let sequence_number = self.tick();
                    if head.text_input {
                        emit(
                            context,
                            E::CustomToolInputDone(wire::InputDone {
                                input: arguments.clone(),
                                item_id: id,
                                output_index: index,
                                sequence_number,
                                extra: Default::default(),
                            }),
                        );
                    } else {
                        emit(
                            context,
                            E::FunctionArgumentsDone(wire::ArgumentsDone {
                                arguments: arguments.clone(),
                                item_id: id,
                                output_index: index,
                                sequence_number,
                                extra: Default::default(),
                            }),
                        );
                    }
                    let mut item = item;
                    match &mut item {
                        OutputItem::FunctionCall(call) => call.arguments = arguments.clone(),
                        OutputItem::Other(fields) => {
                            fields.insert("input".into(), Value::String(arguments.clone()));
                        }
                        _ => unreachable!("这里只构造工具项"),
                    }
                    self.parts.insert(*key, Entry { index, item });
                } else {
                    let sequence_number = self.tick();
                    let entry = self.entry_mut(*key)?;
                    let id = item_id(&entry.item)?;
                    let text = text_mut(&mut entry.item)?.clone();
                    let index = entry.index;
                    match &context.part(*key)?.head {
                        Head::Reasoning => emit(
                            context,
                            E::ReasoningSummaryTextDone(wire::SummaryDone {
                                item_id: id,
                                output_index: index,
                                sequence_number,
                                summary_index: 0,
                                text,
                                extra: Default::default(),
                            }),
                        ),
                        Head::Refusal => emit(
                            context,
                            E::RefusalDone(wire::RefusalDone {
                                content_index: 0,
                                item_id: id,
                                output_index: index,
                                sequence_number,
                                refusal: text,
                                extra: Default::default(),
                            }),
                        ),
                        _ => emit(
                            context,
                            E::OutputTextDone(wire::TextDone {
                                content_index: 0,
                                item_id: id,
                                logprobs: O::Missing,
                                output_index: index,
                                sequence_number,
                                text,
                                extra: Default::default(),
                            }),
                        ),
                    }
                }
            }
            Event::Annotation { key, annotation }
                if context.source == crate::protocol::Protocol::OpenAiResponses =>
            {
                let bytes = serde_json::to_vec(annotation)
                    .map_err(|_| Error::Invalid("引用叶子无法计量".into()))?
                    .len();
                self.reserve(bytes, context)?;
                let sequence_number = self.tick();
                let entry = self.entry_mut(*key)?;
                let id = item_id(&entry.item)?;
                let OutputItem::Message(message) = &mut entry.item else {
                    return Err(unsupported("annotations", "引用只能写入输出文本"));
                };
                let OutputPart::OutputText { annotations, .. } = &mut message.content[0] else {
                    return Err(unsupported("annotations", "引用只能写入输出文本"));
                };
                let annotation_index = annotations.len() as u64;
                annotations.push(annotation.clone());
                emit(
                    context,
                    E::AnnotationAdded(wire::AnnotationAdded {
                        annotation: O::Value(annotation.clone()),
                        annotation_index,
                        content_index: 0,
                        item_id: id,
                        output_index: entry.index,
                        sequence_number,
                        extra: Default::default(),
                    }),
                );
            }
            Event::Annotation { .. } => {
                context.warn("annotations", "来源引用没有等价的 Responses 定位，已丢弃")
            }
            Event::Signature { .. } => context.warn(
                "signature",
                "Responses 不能使用来源签名，需由接入方保存工具回合状态",
            ),
            Event::CandidateEnd { reason, .. } => {
                if *reason == FinishReason::Unknown {
                    return Err(unsupported("finish_reason", "未知结束原因不能生成完成快照"));
                }
                self.reason = Some(*reason);
                // 引用可能在源内容块关闭后到达，项结束延迟到候选结束，正文增量早已交付。
                let mut keys = self.parts.keys().copied().collect::<Vec<_>>();
                keys.sort_by_key(|key| self.parts[key].index);
                for key in keys {
                    self.close_item(key, context)?;
                }
            }
            Event::End(status) => {
                let failed = matches!(status, Status::Failed | Status::Cancelled);
                let incomplete = *status == Status::Incomplete
                    || matches!(
                        self.reason,
                        Some(FinishReason::Length | FinishReason::Filtered)
                    );
                let state = if failed {
                    "failed"
                } else if incomplete {
                    "incomplete"
                } else {
                    "completed"
                };
                let mut entries = std::mem::take(&mut self.parts)
                    .into_values()
                    .collect::<Vec<_>>();
                entries.sort_by_key(|entry| entry.index);
                let mut response = self.response(
                    state,
                    entries.into_iter().map(|entry| entry.item).collect(),
                    context,
                );
                response.usage = context
                    .usage()?
                    .map(|usage| encode_responses_usage(&usage, None))
                    .transpose()?
                    .into();
                if failed {
                    response.error = O::Value(ResponseError {
                        code: "server_error".into(),
                        message: "Provider generation failed".into(),
                        misalignment: O::Missing,
                        extra: Default::default(),
                    });
                }
                if incomplete {
                    response.incomplete_details = O::Value(IncompleteDetails {
                        reason: if self.reason == Some(FinishReason::Filtered) {
                            "content_filter"
                        } else {
                            "max_output_tokens"
                        }
                        .into(),
                        extra: Default::default(),
                    });
                }
                let lifecycle = wire::Lifecycle {
                    response: Box::new(response),
                    sequence_number: self.tick(),
                    extra: Default::default(),
                };
                emit(
                    context,
                    if failed {
                        E::Failed(lifecycle)
                    } else if incomplete {
                        E::Incomplete(lifecycle)
                    } else {
                        E::Completed(lifecycle)
                    },
                );
                self.bytes = 0;
            }
            Event::Media { .. } => {
                return Err(unsupported(
                    "media",
                    "Responses 没有等价的通用完整媒体输出项",
                ));
            }
            _ => {}
        }
        Ok(())
    }

    /// 一条响应中的事件顺序号严格递增；事件数量受接入层单流时限约束。
    fn tick(&mut self) -> u64 {
        let value = self.sequence;
        self.sequence += 1;
        value
    }

    /// 在复制正文前检查，参数缓冲与快照使用同一总预算。
    fn reserve(&mut self, count: usize, context: &Context<'_>) -> Result<()> {
        let bytes = self
            .bytes
            .checked_add(count)
            .ok_or_else(|| unsupported("stream.buffer", "缓冲长度溢出"))?;
        if bytes > context.remaining {
            return Err(unsupported("stream.buffer", "Responses 快照超过缓冲上限"));
        }
        self.bytes = bytes;
        Ok(())
    }

    /// 找到已经创建的输出项，杜绝隐式创建正文或重复来源索引。
    fn entry_mut(&mut self, key: Key) -> Result<&mut Entry> {
        self.parts
            .get_mut(&key)
            .ok_or_else(|| Error::Invalid("Responses 目标输出项不存在".into()))
    }

    /// 内容完成快照只供协议验证使用，不能再次作为正文追加。
    fn close_item(&mut self, key: Key, context: &mut Context<'_>) -> Result<()> {
        let sequence_number = self.tick();
        let entry = self.entry_mut(key)?;
        let id = item_id(&entry.item)?;
        let index = entry.index;
        match &mut entry.item {
            OutputItem::Message(message) => {
                message.status = ItemStatus::Completed;
                emit(
                    context,
                    E::ContentPartDone(wire::ContentEvent {
                        content_index: 0,
                        item_id: id,
                        output_index: index,
                        part: wire::ContentPart::Output(message.content[0].clone()),
                        sequence_number,
                        extra: Default::default(),
                    }),
                );
            }
            OutputItem::Other(fields)
                if fields.get("type").and_then(Value::as_str) == Some("reasoning") =>
            {
                let text = text_mut(&mut entry.item)?.clone();
                emit(
                    context,
                    E::ReasoningSummaryPartDone(wire::SummaryPart {
                        item_id: id,
                        output_index: index,
                        summary_index: 0,
                        part: wire::ReasoningPart::SummaryText {
                            text,
                            extra: Default::default(),
                        },
                        sequence_number,
                        extra: Default::default(),
                    }),
                );
            }
            _ => {}
        }
        let item = entry.item.clone();
        let sequence_number = self.tick();
        emit(
            context,
            E::OutputItemDone(wire::ItemEvent {
                item,
                output_index: index,
                sequence_number,
                extra: Default::default(),
            }),
        );
        Ok(())
    }

    /// 各生命周期事件统一使用目标外壳。
    fn response(&self, status: &str, output: Vec<OutputItem>, context: &Context<'_>) -> Response {
        Response {
            id: context.target.id.clone(),
            created_at: context.target.created,
            model: context.target.model.clone(),
            object: "response".into(),
            output,
            status: O::Value(status.into()),
            error: O::Null,
            incomplete_details: O::Null,
            ..Default::default()
        }
    }
}

/// 从类型化输出项读取 ID，其他仅允许已构造的推理或自定义工具叶子。
fn item_id(item: &OutputItem) -> Result<String> {
    match item {
        OutputItem::Message(message) => Ok(message.id.clone()),
        OutputItem::FunctionCall(call) => match &call.id {
            O::Value(id) => Ok(id.clone()),
            _ => Err(Error::Invalid("Responses 函数项缺少目标 ID".into())),
        },
        OutputItem::Other(fields) => fields
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| Error::Invalid("Responses 输出项缺少目标 ID".into())),
    }
}

/// 只修改文本或推理叶子，不将完整协议报文投影为 Value。
fn text_mut(item: &mut OutputItem) -> Result<&mut String> {
    let value = match item {
        OutputItem::Message(message) => match &mut message.content[0] {
            OutputPart::OutputText { text, .. } => return Ok(text),
            OutputPart::Refusal { refusal, .. } => return Ok(refusal),
        },
        OutputItem::Other(fields) => fields
            .get_mut("summary")
            .and_then(Value::as_array_mut)
            .and_then(|parts| parts.first_mut())
            .and_then(|part| part.get_mut("text")),
        _ => None,
    };
    match value {
        Some(Value::String(text)) => Ok(text),
        _ => Err(Error::Invalid("Responses 输出项不含文本".into())),
    }
}

/// 已知类型事件直接交给 HTTP 边界序列化。
fn emit(context: &mut Context<'_>, event: E) {
    context
        .events
        .push(Raw::Responses(Box::new(wire::Event::Known(Box::new(
            event,
        )))));
}
