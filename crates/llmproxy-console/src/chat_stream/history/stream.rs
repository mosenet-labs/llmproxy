//! 事件 IR 汇总为本轮历史；未正常结束的汇总不会提交给会话。
use super::Content;
use llmproxy_core::{
    ir::{
        message::{Part, PartKind, Role, ToolCall},
        request::{Item, Message},
        stream::{Event, Head, Key, ToolHead},
    },
    protocol::Protocol,
};
use std::collections::BTreeMap;

struct Pending {
    part: Part,
    tool: Option<ToolHead>,
    arguments: String,
}

/// 仅收集页面选中的候选，签名不进入可见文字。
#[derive(Default)]
pub(in crate::chat_stream) struct Collector {
    parts: BTreeMap<Key, Pending>,
    bytes: usize,
}
impl Collector {
    /// 每个事件只消费一次；内容容量与页面正文使用同一上限。
    pub fn push(&mut self, event: &Event) -> Result<(), String> {
        let added = match event {
            Event::PartStart {
                key,
                head: Head::Tool(tool),
            } if key.candidate == 0 => {
                tool.id.as_ref().map_or(0, String::len) + tool.name.as_ref().map_or(0, String::len)
            }
            Event::TextDelta { key, text } if key.candidate == 0 => text.len(),
            Event::ToolDelta {
                key,
                id,
                name,
                arguments,
            } if key.candidate == 0 => {
                id.as_ref().map_or(0, String::len)
                    + name.as_ref().map_or(0, String::len)
                    + arguments.as_ref().map_or(0, String::len)
            }
            Event::Signature { key, data, .. } if key.candidate == 0 => data.len(),
            Event::Media { key, media } if key.candidate == 0 => match &media.source {
                llmproxy_core::ir::media::MediaSource::Base64 { data, .. } => data.len(),
                llmproxy_core::ir::media::MediaSource::Url { uri, .. } => uri.len(),
                llmproxy_core::ir::media::MediaSource::FileId(id)
                | llmproxy_core::ir::media::MediaSource::Text(id) => id.len(),
            },
            Event::ServerOutput { key, output } if key.candidate == 0 => serde_json::to_vec(output)
                .map_err(|_| "无法保存响应历史")?
                .len(),
            _ => 0,
        };
        self.bytes = self
            .bytes
            .checked_add(added)
            .filter(|bytes| *bytes <= super::super::MAX_REPLY_BYTES)
            .ok_or("上游回复过长")?;
        match event {
            Event::PartStart { key, head } if key.candidate == 0 => {
                let (kind, tool) = match head {
                    Head::Text => (PartKind::Text(String::new()), None),
                    Head::Refusal => (PartKind::Refusal(String::new()), None),
                    Head::Reasoning => (PartKind::Reasoning("".into()), None),
                    Head::Tool(tool) => (
                        PartKind::ToolCall(ToolCall {
                            id: tool.id.clone(),
                            name: tool.name.clone().unwrap_or_default(),
                            arguments: "".into(),
                        }),
                        Some(tool.clone()),
                    ),
                    Head::Native(_) => return Ok(()),
                };
                self.parts.insert(
                    *key,
                    Pending {
                        part: Part {
                            kind,
                            metadata: Default::default(),
                        },
                        tool,
                        arguments: String::new(),
                    },
                );
            }
            Event::TextDelta { key, text } if key.candidate == 0 => {
                if let Some(pending) = self.parts.get_mut(key) {
                    match &mut pending.part.kind {
                        PartKind::Text(current) | PartKind::Refusal(current) => {
                            current.push_str(text)
                        }
                        PartKind::Reasoning(current) => {
                            let mut value = current.as_str().unwrap_or_default().to_owned();
                            value.push_str(text);
                            *current = value.into();
                        }
                        _ => {}
                    }
                }
            }
            Event::ToolDelta {
                key,
                id,
                name,
                arguments,
            } if key.candidate == 0 => {
                if let Some(pending) = self.parts.get_mut(key)
                    && let Some(tool) = &mut pending.tool
                {
                    if tool.id.is_none() {
                        tool.id.clone_from(id);
                    }
                    if let Some(name) = name {
                        tool.name.get_or_insert_with(String::new).push_str(name);
                    }
                    if let Some(arguments) = arguments {
                        pending.arguments.push_str(arguments);
                    }
                }
            }
            Event::Signature {
                key,
                protocol,
                data,
            } if key.candidate == 0 => {
                if let Some(pending) = self.parts.get_mut(key)
                    && *protocol == Protocol::Gemini
                {
                    pending.part.metadata.insert("_llmproxy_wire".into(), serde_json::json!({"protocol":protocol.as_str(),"form":"part","extra":{"thoughtSignature":data}}));
                }
            }
            Event::PartEnd(key) if key.candidate == 0 => {
                if let Some(pending) = self.parts.get_mut(key)
                    && let Some(tool) = pending.tool.take()
                {
                    pending.part.kind = PartKind::ToolCall(ToolCall {
                        id: tool.id,
                        name: tool.name.unwrap_or_default(),
                        arguments: if tool.text_input {
                            pending.arguments.clone().into()
                        } else {
                            serde_json::from_str(&pending.arguments)
                                .unwrap_or_else(|_| pending.arguments.clone().into())
                        },
                    });
                }
            }
            Event::Media { key, media } if key.candidate == 0 => {
                self.insert(*key, PartKind::Media(media.clone()));
            }
            Event::ServerOutput { key, output } if key.candidate == 0 => {
                self.insert(*key, PartKind::ServerOutput(output.clone()));
            }
            _ => {}
        }
        Ok(())
    }

    /// 本轮完整结束后才返回结构化内容，保持输出项及内容块的顺序。
    pub fn finish(self) -> Content {
        let mut content = Content::default();
        let mut current = None;
        for (key, pending) in self.parts {
            if current != Some(key.item) {
                let index = content.messages.len();
                content.messages.push(Message {
                    role: Role::Assistant,
                    parts: Vec::new(),
                    metadata: Default::default(),
                });
                content.items.push(Item::Message(index));
                current = Some(key.item);
            }
            content
                .messages
                .last_mut()
                .unwrap()
                .parts
                .push(pending.part);
        }
        content
    }

    /// 完整媒体与服务端结果使用相同内容块保存路径。
    fn insert(&mut self, key: Key, kind: PartKind) {
        self.parts.insert(
            key,
            Pending {
                part: Part {
                    kind,
                    metadata: Default::default(),
                },
                tool: None,
                arguments: String::new(),
            },
        );
    }
}
