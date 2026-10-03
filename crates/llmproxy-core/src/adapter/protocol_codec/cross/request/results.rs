//! 工具结果中的文本与媒体单独映射，不能把媒体内容块序列化成普通文本。
//! 参考：https://ai.google.dev/api/caching#FunctionResponse
//! 参考：https://platform.claude.com/docs/en/agents-and-tools/tool-use/implement-tool-use
use super::super::{ConversionWarning, unsupported, warn};
use super::items::{Items, push_gemini_part, push_message_block};
use crate::{
    adapter::{Result, media, wire},
    ir::{
        media::{Media, OriginalMedia as Raw},
        message::ToolResult,
    },
    protocol::{
        OptionalNullable as O, Protocol,
        chat::request as c,
        gemini::request as g,
        messages::request as m,
        responses::{function as rf, request as r},
    },
};
use serde_json::{Map, Value};

/// 局部结果片段进入通用媒体语义；任意 JSON 结果仍作为动态工具数据保存。
enum Part {
    Text(String),
    Media(Media),
}

/// 已配对的调用结果按目标类型构造；错误标志通过目标可表达的方式保留。
pub(super) fn emit(
    items: &mut Items,
    id: String,
    name: String,
    result: &ToolResult,
    metadata: Option<&Map<String, Value>>,
    source: Protocol,
    warnings: &mut Vec<ConversionWarning>,
) -> Result<()> {
    let target = match items {
        Items::Chat(_) => Protocol::OpenAiChat,
        Items::Responses(_) => Protocol::OpenAiResponses,
        Items::Messages(_) => Protocol::AnthropicMessages,
        Items::Gemini(_) => Protocol::Gemini,
    };
    let extra = metadata.map(|m| wire::extra(m, source)).unwrap_or_default();
    let is_error = extra.get("is_error").and_then(Value::as_bool).or_else(|| {
        (source == Protocol::Gemini && result.content.get("error").is_some()).then_some(true)
    });
    // 已映射的错误标志和媒体不能再误报为丢弃，其他附加字段继续报告。
    let mut residual = metadata.cloned().unwrap_or_default();
    if let Some(fields) = residual
        .get_mut("_llmproxy_wire")
        .and_then(Value::as_object_mut)
        .and_then(|w| w.get_mut("extra"))
        .and_then(Value::as_object_mut)
    {
        fields.remove("is_error");
        if let Some(nested) = fields
            .get_mut("functionResponse")
            .and_then(Value::as_object_mut)
        {
            nested.remove("parts");
            nested.remove("willContinue");
            if nested.is_empty() {
                fields.remove("functionResponse");
            }
        }
    }
    super::super::warn_metadata(&residual, source, target, "tool.result", warnings)?;
    let mut parts = Vec::new();
    if result.content.is_array()
        && matches!(
            source,
            Protocol::AnthropicMessages | Protocol::OpenAiResponses
        )
    {
        for value in result.content.as_array().unwrap() {
            let raw = if source == Protocol::AnthropicMessages {
                let block: m::ContentBlock = serde_json::from_value(value.clone())?;
                if let m::ContentBlock::Known(m::KnownContentBlock::Text { text, .. }) = block {
                    parts.push(Part::Text(text));
                    continue;
                }
                Raw::Messages(block)
            } else {
                let block: r::InputPart = serde_json::from_value(value.clone())?;
                if let r::InputPart::InputText { text, .. } = block {
                    parts.push(Part::Text(text));
                    continue;
                }
                Raw::Responses(block)
            };
            parts.push(Part::Media(media::decode(&raw).ok_or_else(|| {
                unsupported("tool.result", "工具结果包含无法映射的内容块")
            })?));
        }
    } else {
        parts.push(Part::Text(match &result.content {
            Value::String(text) => text.clone(),
            Value::Null => String::new(),
            value => serde_json::to_string(value)?,
        }));
    }
    if let Some(result_extra) = extra.get("functionResponse") {
        if result_extra.get("willContinue").and_then(Value::as_bool) == Some(true) {
            return Err(unsupported(
                "tool.result.willContinue",
                "未完成的非阻塞工具结果不能转为已完成结果",
            ));
        }
        if let Some(raw_parts) = result_extra.get("parts").and_then(Value::as_array) {
            for part in raw_parts {
                let raw = Raw::Gemini(Box::new(serde_json::from_value(part.clone())?));
                parts.push(Part::Media(media::decode(&raw).ok_or_else(|| {
                    unsupported("tool.result.parts", "工具结果包含无法映射的媒体")
                })?));
            }
        }
    }
    let text = parts
        .iter()
        .filter_map(|p| match p {
            Part::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let has_media = parts.iter().any(|p| matches!(p, Part::Media(_)));
    if is_error == Some(true) && matches!(target, Protocol::OpenAiChat | Protocol::OpenAiResponses)
    {
        warn(
            warnings,
            source,
            target,
            "tool.result.is_error",
            "目标无独立错误标志，结果正文附加工具执行失败说明",
        );
    }
    let text = if is_error == Some(true)
        && matches!(target, Protocol::OpenAiChat | Protocol::OpenAiResponses)
    {
        format!("Tool execution failed: {text}")
    } else {
        text
    };
    match items {
        Items::Chat(items) => {
            if has_media {
                return Err(unsupported(
                    "tool.result.media",
                    "Chat 工具结果只支持文本，不能把媒体伪装为文本",
                ));
            }
            items.push(c::Message::Tool {
                tool_call_id: id,
                content: c::Content::Text(text),
                extra: Map::new(),
            });
        }
        Items::Responses(items) => {
            let output = if has_media {
                let mut blocks = Vec::new();
                if is_error == Some(true) {
                    blocks.push(r::InputPart::InputText {
                        text: "Tool execution failed".into(),
                        extra: Map::new(),
                    });
                }
                for part in parts {
                    blocks.push(match part {
                        Part::Text(text) => r::InputPart::InputText {
                            text,
                            extra: Map::new(),
                        },
                        Part::Media(media) => match encode_media(media, target)? {
                            Raw::Responses(part) => part,
                            _ => unreachable!(),
                        },
                    });
                }
                // CallOutput.output 是协议允许的动态结果叶子，不是整包报文中转。
                serde_json::to_value(blocks)?
            } else {
                Value::String(text)
            };
            items.push(r::body::InputItem::FunctionCallOutput(rf::CallOutput {
                r#type: rf::ResultType::FunctionCallOutput,
                call_id: O::Value(id),
                output,
                id: O::Missing,
                status: O::Missing,
                extra: Map::new(),
            }));
        }
        Items::Messages(items) => {
            let content = if has_media {
                let mut blocks = Vec::new();
                for part in parts {
                    blocks.push(match part {
                        Part::Text(text) => m::ContentBlock::Known(m::KnownContentBlock::Text {
                            text,
                            cache_control: O::Missing,
                            citations: O::Missing,
                            extra: Map::new(),
                        }),
                        Part::Media(media) => match encode_media(media, target)? {
                            Raw::Messages(part) => part,
                            _ => unreachable!(),
                        },
                    });
                }
                m::ToolResultContent::Parts(blocks)
            } else {
                m::ToolResultContent::Text(text)
            };
            push_message_block(
                items,
                m::Role::User,
                m::ContentBlock::Known(m::KnownContentBlock::ToolResult {
                    tool_use_id: id,
                    content: O::Value(content),
                    is_error: is_error.into(),
                    cache_control: O::Missing,
                    toolset_name: O::Missing,
                    extra: Map::new(),
                }),
            );
        }
        Items::Gemini(items) => {
            let mut response = result
                .content
                .as_object()
                .cloned()
                .unwrap_or_else(|| Map::from_iter([("result".into(), Value::String(text))]));
            if is_error == Some(true) && !response.contains_key("error") {
                response = Map::from_iter([("error".into(), Value::Object(response))]);
            }
            let mut media_parts = Vec::new();
            for part in parts {
                if let Part::Media(media) = part {
                    match encode_media(media, target)? {
                        Raw::Gemini(part) => media_parts.push(serde_json::to_value(part)?),
                        _ => unreachable!(),
                    }
                }
            }
            if has_media {
                warn(
                    warnings,
                    source,
                    target,
                    "tool.result.parts",
                    "工具结果文本置于 response，媒体置于 parts，二者交错顺序无法保持",
                );
            }
            push_gemini_part(
                items,
                g::Role::User,
                g::Part {
                    function_response: O::Value(g::FunctionResponse {
                        id: O::Value(id),
                        name,
                        response,
                        parts: if media_parts.is_empty() {
                            O::Missing
                        } else {
                            O::Value(media_parts)
                        },
                        will_continue: O::Missing,
                        scheduling: O::Missing,
                        extra: Map::new(),
                    }),
                    ..Default::default()
                },
            );
        }
    }
    Ok(())
}

/// 跨协议仅根据媒体公共字段编码，绝不复用来源副本。
fn encode_media(mut value: Media, target: Protocol) -> Result<Raw> {
    value.original = None;
    media::encode(&value, target)
}
