//! 将 IR 断点写到刚生成的目标内容块，避免把中间断点移动到整条请求末尾。
use super::items::Items;
use crate::protocol::messages::request::cache::CacheControl;
use crate::protocol::{
    OptionalNullable as O, chat::request as c, messages::request as m, responses::request as r,
};
use serde_json::{Map, Value};

impl Items {
    /// 返回 false 表示该目标项没有可承载内容断点的位置。
    pub(super) fn cache_breakpoint(&mut self, control: CacheControl) -> bool {
        match self {
            Self::Chat(items) => {
                let Some(message) = items.last_mut() else {
                    return false;
                };
                match message {
                    c::Message::System(message) | c::Message::Developer(message) => {
                        return text_marker(&mut message.content);
                    }
                    c::Message::Tool { content, .. } => return text_marker(content),
                    c::Message::User(message) => {
                        if let c::Content::Text(text) = &mut message.content {
                            message.content = c::Content::Parts(vec![c::UserPart::Text {
                                text: std::mem::take(text),
                                prompt_cache_breakpoint: O::Missing,
                                extra: Map::new(),
                            }]);
                        }
                        if let c::Content::Parts(parts) = &mut message.content
                            && let Some(part) = parts.last_mut()
                        {
                            match part {
                                c::UserPart::Text {
                                    prompt_cache_breakpoint,
                                    ..
                                }
                                | c::UserPart::ImageUrl {
                                    prompt_cache_breakpoint,
                                    ..
                                }
                                | c::UserPart::InputAudio {
                                    prompt_cache_breakpoint,
                                    ..
                                }
                                | c::UserPart::File {
                                    prompt_cache_breakpoint,
                                    ..
                                } => *prompt_cache_breakpoint = marker(),
                            }
                            return true;
                        }
                    }
                    c::Message::Assistant {
                        content: O::Value(content),
                        ..
                    } => {
                        if let c::Content::Text(text) = content {
                            *content = c::Content::Parts(vec![c::AssistantPart::Text {
                                text: std::mem::take(text),
                                prompt_cache_breakpoint: O::Missing,
                                extra: Map::new(),
                            }]);
                        }
                        if let c::Content::Parts(parts) = content
                            && let Some(c::AssistantPart::Text {
                                prompt_cache_breakpoint,
                                ..
                            }) = parts.last_mut()
                        {
                            *prompt_cache_breakpoint = marker();
                            return true;
                        }
                    }
                    _ => {}
                }
            }
            Self::Responses(items) => {
                if let Some(r::body::InputItem::Message(r::Message::Easy(message))) =
                    items.last_mut()
                {
                    if let r::Content::Text(text) = &mut message.content {
                        message.content = r::Content::Parts(vec![r::InputPart::InputText {
                            text: std::mem::take(text),
                            extra: Map::new(),
                        }]);
                    }
                    if let r::Content::Parts(parts) = &mut message.content
                        && let Some(part) = parts.last_mut()
                    {
                        let extra = match part {
                            r::InputPart::InputText { extra, .. }
                            | r::InputPart::InputImage { extra, .. }
                            | r::InputPart::InputFile { extra, .. } => extra,
                        };
                        extra.insert(
                            "prompt_cache_breakpoint".into(),
                            serde_json::json!({"mode":"explicit"}),
                        );
                        return true;
                    }
                }
            }
            Self::Messages(items) => {
                if let Some(message) = items.last_mut() {
                    if let m::Content::Text(text) = &mut message.content {
                        message.content = m::Content::Parts(vec![m::ContentBlock::Known(
                            m::KnownContentBlock::Text {
                                text: std::mem::take(text),
                                cache_control: O::Missing,
                                citations: O::Missing,
                                extra: Map::new(),
                            },
                        )]);
                    }
                    if let m::Content::Parts(parts) = &mut message.content
                        && let Some(m::ContentBlock::Known(part)) = parts.last_mut()
                    {
                        let field = match part {
                            m::KnownContentBlock::Text { cache_control, .. }
                            | m::KnownContentBlock::Image { cache_control, .. }
                            | m::KnownContentBlock::Document { cache_control, .. }
                            | m::KnownContentBlock::ToolUse { cache_control, .. }
                            | m::KnownContentBlock::ToolResult { cache_control, .. } => {
                                cache_control
                            }
                            _ => return false,
                        };
                        *field = O::Value(control);
                        return true;
                    }
                }
            }
            Self::Gemini(_) => {}
        }
        false
    }
}
/// Chat 的内容标记不承载 TTL；TTL 的降级由调用方统一记录。
fn marker() -> O<c::PromptCacheBreakpoint> {
    O::Value(c::PromptCacheBreakpoint {
        mode: "explicit".into(),
        extra: Map::<String, Value>::new(),
    })
}

/// system、developer 和 tool 共用文本内容类型，缓存边界落在最后一个文本块。
fn text_marker(content: &mut c::Content<c::TextPart>) -> bool {
    if let c::Content::Text(text) = content {
        *content = c::Content::Parts(vec![c::TextPart::Text {
            text: std::mem::take(text),
            prompt_cache_breakpoint: O::Missing,
            extra: Map::new(),
        }]);
    }
    if let c::Content::Parts(parts) = content
        && let Some(c::TextPart::Text {
            prompt_cache_breakpoint,
            ..
        }) = parts.last_mut()
    {
        *prompt_cache_breakpoint = marker();
        true
    } else {
        false
    }
}
