//! 页面与 Gateway 共用 SSE 分帧及事件 IR，逐帧显示并校验正常结束。
use super::{ChatReply, DisplayPart, MAX_REPLY_BYTES, audio::Audio, display};
use llmproxy_core::{
    adapter::protocol_codec::{ProtocolCodec, stream::Decoder},
    ir::{
        response::Status,
        stream::{Event, Head, Key, Limits},
    },
    protocol::{
        Protocol,
        stream::{self, sse},
    },
};
use std::collections::BTreeMap;

pub(super) struct Reply {
    protocol: Protocol,
    framing: sse::Decoder,
    decoder: Decoder,
    heads: BTreeMap<Key, Head>,
    texts: BTreeMap<Key, usize>,
    tools: BTreeMap<Key, (String, usize)>,
    audio: Audio,
    reply: ChatReply,
}
impl Reply {
    /// 容量按页面可展示正文限制；不保存不透明签名和原始协议副本。
    pub(super) fn new(protocol: Protocol) -> Self {
        Self {
            protocol,
            framing: sse::Decoder::new(MAX_REPLY_BYTES),
            decoder: protocol.stream_decoder(Limits {
                buffered_bytes: MAX_REPLY_BYTES,
                entries: 1024,
            }),
            heads: BTreeMap::new(),
            texts: BTreeMap::new(),
            tools: BTreeMap::new(),
            audio: Audio::default(),
            reply: ChatReply::default(),
        }
    }
    /// 已完成的每一帧触发页面更新，包括没有文字变化的用量和工具更新。
    pub(super) fn push(
        &mut self,
        bytes: &[u8],
        update: &mut impl FnMut(&ChatReply),
    ) -> Result<(), String> {
        for byte in bytes {
            if let Some(frame) = self.framing.push(*byte).map_err(str::to_owned)? {
                let raw = sse::decode(self.protocol, &frame)
                    .map_err(|_| "Provider 生成失败".to_owned())?;
                self.raw(&raw)?;
                self.check_limit()?;
                update(&self.reply);
            }
        }
        Ok(())
    }
    /// HTTP 正常结束后检查协议生命周期；截断文字不进入下一轮历史。
    pub(super) fn finish(mut self) -> Result<ChatReply, String> {
        self.framing.finish().map_err(str::to_owned)?;
        self.raw(&stream::Event::End(self.protocol))?;
        match self.decoder.state().ended() {
            Some(Status::Completed | Status::Incomplete) => {}
            _ => return Err("Provider 未正常完成本次生成".into()),
        }
        if self.reply.content.is_empty()
            && self.reply.thinking.is_empty()
            && self.reply.parts.is_empty()
        {
            return Err("上游未返回可显示的内容".into());
        }
        Ok(self.reply)
    }
    /// 将类型化事件投影到页面的有序内容；签名和私有资源不进入展示数据。
    fn raw(&mut self, raw: &stream::Event) -> Result<(), String> {
        // 保留现有 Chat 兼容接口的可见推理扩展；开放叶子不经过整包 Value。
        if let stream::Event::Chat(chunk) = raw {
            for choice in &chunk.choices {
                if choice.index == 0
                    && let Some(text) = choice
                        .delta
                        .extra
                        .get("reasoning_content")
                        .and_then(serde_json::Value::as_str)
                {
                    self.reply.thinking.push_str(text);
                }
            }
        }
        for event in self
            .decoder
            .push(raw)
            .map_err(|_| "无法解码上游流式响应".to_owned())?
        {
            match event {
                Event::PartStart { key, head } if key.candidate == 0 => {
                    if let Head::Tool(tool) = &head {
                        let index = self.reply.parts.len();
                        let name = tool.name.clone().unwrap_or_default();
                        self.reply.parts.push(DisplayPart {
                            title: format!("工具调用 · {name}"),
                            text: String::new(),
                            media: None,
                        });
                        self.tools.insert(key, (name, index));
                    }
                    self.heads.insert(key, head);
                }
                Event::TextDelta { key, text } if key.candidate == 0 => {
                    if matches!(self.heads.get(&key), Some(Head::Reasoning)) {
                        self.reply.thinking.push_str(&text);
                    } else {
                        self.reply.content.push_str(&text);
                        let index = *self.texts.entry(key).or_insert_with(|| {
                            let index = self.reply.parts.len();
                            self.reply.parts.push(DisplayPart {
                                title: String::new(),
                                text: String::new(),
                                media: None,
                            });
                            index
                        });
                        self.reply.parts[index].text.push_str(&text);
                    }
                }
                Event::ToolDelta {
                    key,
                    name,
                    arguments,
                    ..
                } if key.candidate == 0 => {
                    if let Some((current, index)) = self.tools.get_mut(&key) {
                        if let Some(name) = name {
                            current.push_str(&name);
                            self.reply.parts[*index].title = format!("工具调用 · {current}");
                        }
                        if let Some(arguments) = arguments {
                            self.reply.parts[*index].text.push_str(&arguments);
                        }
                    }
                }
                Event::PartEnd(key) => {
                    self.heads.remove(&key);
                    self.tools.remove(&key);
                }
                Event::Media { key, media } if key.candidate == 0 => {
                    display::media_part(&media, &mut self.reply)
                }
                Event::ServerOutput { key, output } if key.candidate == 0 => {
                    display::server_part(&output, &mut self.reply)
                }
                Event::Native(event) => self.audio.push(&event, &mut self.reply)?,
                Event::Failure(_) => return Err("Provider 生成失败".into()),
                _ => {}
            }
        }
        self.reply.usage = self.decoder.state().usage().snapshot().cloned();
        Ok(())
    }
    /// 普通文本已经计入 content；工具和媒体按各自展示载体计入预算。
    fn check_limit(&self) -> Result<(), String> {
        let extra: usize = self
            .reply
            .parts
            .iter()
            .filter(|p| !p.title.is_empty())
            .map(|p| p.title.len() + p.text.len() + p.media.as_ref().map_or(0, |m| m.uri.len()))
            .sum();
        if self.reply.content.len()
            + self.reply.thinking.len()
            + self.reply.summary.len()
            + extra
            + self.audio.buffered_bytes()
            > MAX_REPLY_BYTES
        {
            Err("上游回复过长".into())
        } else {
            Ok(())
        }
    }
}
