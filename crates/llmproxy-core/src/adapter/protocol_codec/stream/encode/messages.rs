//! Messages 的消息、内容块和累计统计分别生成；不使用全量响应等待首段文字。
//! 参考：https://platform.claude.com/docs/en/build-with-claude/streaming
use std::collections::BTreeMap;

use super::{Context, finish, unsupported};
use crate::{
    adapter::{Error, Result, response::encode_messages_usage},
    ir::{
        response::Status,
        stream::{Event, Head, Key},
    },
    protocol::{
        OptionalNullable as O, Protocol,
        messages::response::{
            Message,
            event::{self as wire, KnownDelta as D, KnownEvent as E},
            message::{AssistantRole, ContentBlock, KnownContentBlock as Block, MessageType},
            usage::Usage,
        },
        stream::Event as Raw,
    },
};

/// 块索引在真正发送开始事件时分配；不缓存已交付文字。
#[derive(Default)]
pub(super) struct Encoder {
    blocks: BTreeMap<Key, (u64, bool)>,
    next: u64,
}

impl Encoder {
    /// 工具等待名字和参数完整后发出完整头及 JSON 增量，正文仍立即发送。
    pub(super) fn encode(&mut self, event: &Event, context: &mut Context<'_>) -> Result<()> {
        match event {
            Event::Start(_) => emit(
                context,
                E::MessageStart(wire::MessageStart {
                    message: Message {
                        r#type: MessageType::Message,
                        id: context.target.id.clone(),
                        container: O::Missing,
                        content: Vec::new(),
                        diagnostics: O::Missing,
                        model: context.target.model.clone(),
                        role: AssistantRole::Assistant,
                        stop_details: O::Missing,
                        stop_reason: O::Null,
                        stop_sequence: O::Null,
                        // 协议的必填初始快照使用占位值，真实用量仅从 IR Usage 覆盖。
                        usage: Usage {
                            input_tokens: 0,
                            output_tokens: 0,
                            cache_creation: O::Missing,
                            cache_creation_input_tokens: O::Missing,
                            cache_read_input_tokens: O::Missing,
                            inference_geo: O::Missing,
                            output_tokens_details: O::Missing,
                            server_tool_use: O::Missing,
                            service_tier: O::Missing,
                            extra: Default::default(),
                        },
                        extra: Default::default(),
                    },
                    extra: Default::default(),
                }),
            ),
            Event::PartStart { key, head } => match head {
                Head::Tool(head) if head.text_input => {
                    return Err(unsupported(
                        "tool.input",
                        "Messages 不能表达自由文本工具输入",
                    ));
                }
                Head::Tool(_) => {}
                Head::Text | Head::Refusal | Head::Reasoning => {
                    let thinking =
                        *head == Head::Reasoning && context.source == Protocol::AnthropicMessages;
                    if *head == Head::Reasoning && !thinking {
                        context.warn(
                            "reasoning",
                            "来源思考文本作为普通文本保留，不能制造 Messages 思考签名",
                        );
                    }
                    let content_block = if thinking {
                        Block::Thinking {
                            signature: String::new(),
                            thinking: String::new(),
                            extra: Default::default(),
                        }
                    } else {
                        Block::Text {
                            text: String::new(),
                            citations: O::Missing,
                            extra: Default::default(),
                        }
                    };
                    self.start(*key, thinking, content_block, context);
                }
                Head::Native(_) => unreachable!("原生内容由共用边界拒绝"),
            },
            Event::TextDelta { key, text } => {
                let (index, thinking) = self.block(*key)?;
                delta(
                    context,
                    index,
                    if thinking {
                        D::ThinkingDelta {
                            thinking: text.clone(),
                            extra: Default::default(),
                        }
                    } else {
                        D::TextDelta {
                            text: text.clone(),
                            extra: Default::default(),
                        }
                    },
                );
            }
            Event::PartEnd(key) => {
                let current = context.part(*key)?;
                if let Head::Tool(head) = &current.head {
                    context.arguments(current)?;
                    let input = current.arguments.clone();
                    let block = Block::ToolUse {
                        id: context.call_id(*key, head),
                        caller: O::Missing,
                        input: Default::default(),
                        name: head.name.clone().unwrap_or_default(),
                        extra: Default::default(),
                    };
                    let index = self.start(*key, false, block, context);
                    delta(
                        context,
                        index,
                        D::InputJsonDelta {
                            partial_json: input,
                            extra: Default::default(),
                        },
                    );
                }
                let (index, _) = self.block(*key)?;
                emit(
                    context,
                    E::ContentBlockStop(wire::ContentBlockStop {
                        index,
                        extra: Default::default(),
                    }),
                );
            }
            Event::Signature {
                key,
                protocol,
                data,
            } if *protocol == Protocol::AnthropicMessages
                && context.source == Protocol::AnthropicMessages =>
            {
                let (index, thinking) = self.block(*key)?;
                if !thinking {
                    return Err(unsupported("signature", "Messages 签名只能写入思考块"));
                }
                delta(
                    context,
                    index,
                    D::SignatureDelta {
                        signature: data.clone(),
                        extra: Default::default(),
                    },
                );
            }
            Event::Signature { .. } => context.warn(
                "signature",
                "来源签名不能用于 Messages，需由接入方保存工具回合状态",
            ),
            Event::Annotation { key, annotation }
                if context.source == Protocol::AnthropicMessages =>
            {
                let (index, thinking) = self.block(*key)?;
                if thinking || context.part(*key)?.ended {
                    return Err(unsupported(
                        "citations",
                        "Messages 引用必须在文本块关闭前发送",
                    ));
                }
                delta(
                    context,
                    index,
                    D::CitationsDelta {
                        citation: annotation.clone(),
                        extra: Default::default(),
                    },
                );
            }
            Event::Annotation { .. } => context.warn(
                "annotations",
                "来源引用没有等价的 Messages 文档定位，已丢弃",
            ),
            Event::CandidateEnd { reason, .. } => {
                if *reason == crate::ir::response::FinishReason::Filtered {
                    context.warn(
                        "finish_reason",
                        "Messages 没有等价过滤结束分类，使用 refusal",
                    );
                }
                emit(
                    context,
                    E::MessageDelta(wire::MessageDeltaEvent {
                        delta: wire::MessageDelta {
                            stop_reason: O::Value(
                                finish(Protocol::AnthropicMessages, *reason)?.into(),
                            ),
                            stop_sequence: O::Null,
                            extra: Default::default(),
                        },
                        usage: O::Missing,
                        extra: Default::default(),
                    }),
                );
            }
            Event::Usage(_) => {
                if let Some(usage) = context.usage()? {
                    // 部分累计快照不能补造另一项计数；完整用量到达时才生成必需字段。
                    if usage.input_tokens.is_some() && usage.output_tokens.is_some() {
                        let usage = encode_messages_usage(&usage, None)?;
                        emit(
                            context,
                            E::MessageDelta(wire::MessageDeltaEvent {
                                delta: Default::default(),
                                usage: O::Value(wire::DeltaUsage {
                                    cache_creation: usage.cache_creation,
                                    cache_creation_input_tokens: usage.cache_creation_input_tokens,
                                    cache_read_input_tokens: usage.cache_read_input_tokens,
                                    input_tokens: O::Value(usage.input_tokens),
                                    output_tokens: O::Value(usage.output_tokens),
                                    output_tokens_details: usage.output_tokens_details,
                                    server_tool_use: usage.server_tool_use,
                                    extra: Default::default(),
                                }),
                                extra: Default::default(),
                            }),
                        );
                    }
                }
            }
            Event::End(Status::Completed | Status::Incomplete) => {
                let usage = context
                    .usage()?
                    .ok_or_else(|| unsupported("usage", "Messages 结束前必须收到真实用量"))?;
                encode_messages_usage(&usage, None)?;
                emit(context, E::MessageStop(Default::default()));
            }
            Event::Failure(_) => emit(
                context,
                E::Error(wire::ErrorEvent {
                    error: wire::StreamError {
                        r#type: "api_error".into(),
                        message: "Provider generation failed".into(),
                        extra: Default::default(),
                    },
                    extra: Default::default(),
                }),
            ),
            Event::End(Status::Failed | Status::Cancelled) => {}
            Event::Heartbeat => emit(context, E::Ping(Default::default())),
            Event::Media { .. } => {
                return Err(unsupported("media", "Messages 输出没有等价的媒体内容块"));
            }
            _ => {}
        }
        Ok(())
    }

    /// 分配发送索引，使晚到的完整工具头不会留下未创建的索引空洞。
    fn start(&mut self, key: Key, thinking: bool, block: Block, context: &mut Context<'_>) -> u64 {
        let index = self.next;
        self.next += 1;
        self.blocks.insert(key, (index, thinking));
        emit(
            context,
            E::ContentBlockStart(wire::ContentBlockStart {
                index,
                content_block: ContentBlock::Known(block),
                extra: Default::default(),
            }),
        );
        index
    }

    /// 读取已经交付的块索引，防止对未创建内容发送增量或结束事件。
    fn block(&self, key: Key) -> Result<(u64, bool)> {
        self.blocks
            .get(&key)
            .copied()
            .ok_or_else(|| Error::Invalid("Messages 目标内容块不存在".into()))
    }
}

/// 将内部已知事件装入公共类型载体。
fn emit(context: &mut Context<'_>, event: E) {
    context
        .events
        .push(Raw::Messages(Box::new(wire::Event::Known(Box::new(event)))));
}

/// 一个内容增量只影响已创建的目标索引。
fn delta(context: &mut Context<'_>, index: u64, delta: D) {
    emit(
        context,
        E::ContentBlockDelta(wire::ContentBlockDelta {
            index,
            delta: wire::Delta::Known(Box::new(delta)),
            extra: Default::default(),
        }),
    );
}
