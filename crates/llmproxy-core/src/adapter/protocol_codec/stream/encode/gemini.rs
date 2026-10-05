//! Gemini 逐块使用 GenerateContentResponse；函数参数必须等完整对象后才可发送。
//! 参考：https://ai.google.dev/api/generate-content#method:-models.streamgeneratecontent
use super::{Context, finish, unsupported};
use crate::{
    adapter::{Error, Result, media, response::encode_gemini_usage},
    ir::{
        media::OriginalMedia,
        response::Status,
        stream::{Event, Head, Key},
    },
    protocol::{
        OptionalNullable as O, Protocol,
        gemini::{
            request::message::{FunctionCall, Part},
            response::{Message, Response, Role, body::Candidate},
        },
        stream::Event as Raw,
    },
};
use std::collections::BTreeMap;

/// 工具签名与尚未完成的参数使用同一 IR 状态预算，不存储普通文本。
#[derive(Default)]
pub(super) struct Encoder {
    signatures: BTreeMap<Key, String>,
}

/// 文本即时发出；完整工具及媒体叶子重用现有映射，累计统计不求和。
pub(super) fn encode(
    encoder: &mut Encoder,
    event: &Event,
    context: &mut Context<'_>,
) -> Result<()> {
    match event {
        Event::TextDelta { key, text } => {
            let head = &context.part(*key)?.head;
            if !matches!(head, Head::Text | Head::Refusal | Head::Reasoning) {
                return Err(Error::Invalid("Gemini 文本块类型错误".into()));
            }
            part(
                context,
                key.candidate,
                Part {
                    text: O::Value(text.clone()),
                    thought: if matches!(head, Head::Reasoning) {
                        O::Value(true)
                    } else {
                        O::Missing
                    },
                    ..Default::default()
                },
            );
        }
        Event::PartEnd(key) => {
            let current = context.part(*key)?;
            if let Head::Tool(head) = &current.head {
                let args = context.arguments(current)?;
                part(
                    context,
                    key.candidate,
                    Part {
                        function_call: O::Value(FunctionCall {
                            id: O::Value(context.call_id(*key, head)),
                            name: head.name.clone().unwrap_or_default(),
                            args: O::Value(args),
                            extra: Default::default(),
                        }),
                        thought_signature: encoder.signatures.remove(key).into(),
                        ..Default::default()
                    },
                );
            }
        }
        Event::Media {
            key,
            media: content,
        } => match media::encode(content, Protocol::Gemini)? {
            OriginalMedia::Gemini(content) => part(context, key.candidate, *content),
            _ => return Err(Error::Invalid("媒体适配器返回的协议错误".into())),
        },
        Event::Signature {
            key,
            protocol,
            data,
        } if *protocol == Protocol::Gemini && context.source == Protocol::Gemini => {
            // 工具参数尚未交付时暂存签名，在完整调用的同一个 Part 中发送。
            if matches!(context.part(*key)?.head, Head::Tool(_)) {
                if encoder.signatures.get(key).is_some_and(|old| old != data) {
                    return Err(unsupported(
                        "thoughtSignature",
                        "同一 Gemini 工具报告了不同完整签名",
                    ));
                }
                encoder.signatures.insert(*key, data.clone());
                return Ok(());
            }
            part(
                context,
                key.candidate,
                Part {
                    thought_signature: O::Value(data.clone()),
                    ..Default::default()
                },
            );
        }
        Event::Signature { .. } => context.warn(
            "signature",
            "来源签名不能用于 Gemini，需由接入方保存工具回合状态",
        ),
        Event::Annotation { .. } => {
            context.warn("annotations", "来源引用没有等价的 Gemini 增量定位，已丢弃")
        }
        Event::CandidateEnd { index, reason } => {
            if *reason == crate::ir::response::FinishReason::Refusal {
                context.warn(
                    "finish_reason",
                    "Gemini 没有等价的主动拒绝结束分类，正文保留",
                );
            }
            let mut response = empty(context);
            response.candidates = O::Value(vec![Candidate {
                index: O::Value(*index),
                finish_reason: O::Value(finish(Protocol::Gemini, *reason)?.into()),
                ..Default::default()
            }]);
            context.events.push(Raw::Gemini(Box::new(response)));
        }
        Event::Usage(_) => {
            if let Some(usage) = context.usage()? {
                let mut response = empty(context);
                response.usage_metadata = O::Value(encode_gemini_usage(&usage, None));
                context.events.push(Raw::Gemini(Box::new(response)));
            }
        }
        Event::End(Status::Completed | Status::Incomplete) => {
            context.events.push(Raw::End(Protocol::Gemini))
        }
        Event::Failure(_) | Event::End(Status::Failed | Status::Cancelled) => {
            return Err(Error::FailedResponse);
        }
        _ => {}
    }
    Ok(())
}

/// 同一响应的 ID 与模型在各分片中保持一致。
fn empty(context: &Context<'_>) -> Response {
    Response {
        response_id: O::Value(context.target.id.clone()),
        model_version: O::Value(context.target.model.clone()),
        ..Default::default()
    }
}

/// 内容块使用 model 角色，不把模型工具调用转换为客户端工具执行结果。
fn part(context: &mut Context<'_>, index: u64, part: Part) {
    let mut response = empty(context);
    response.candidates = O::Value(vec![Candidate {
        index: O::Value(index),
        content: O::Value(Message {
            role: Some(Role::Model),
            parts: vec![part],
            extra: Default::default(),
        }),
        ..Default::default()
    }]);
    context.events.push(Raw::Gemini(Box::new(response)));
}
