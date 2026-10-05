//! 原生音频仅投影到页面，不将资源 ID、转录或下载地址混入文本历史。
//! 参考：https://developers.openai.com/api/reference/resources/chat/subresources/completions/streaming-events
//! 参考：https://developers.openai.com/api/reference/resources/responses/streaming-events
use super::{ChatReply, DisplayPart, MAX_REPLY_BYTES, display::DisplayMedia};
use base64::{Engine, engine::general_purpose::STANDARD};
use llmproxy_core::{
    ir::media::MediaKind,
    protocol::{
        responses::response::{Event, event::KnownEvent},
        stream,
    },
};

/// 一个可见候选的音频与转录共用收集器；生命周期由协议 decoder 校验。
#[derive(Default)]
pub(super) struct Audio {
    /// 完成前保存解码后的字节，避免直接拼接带 padding 的 Base64 分片。
    bytes: Vec<u8>,
    /// 已创建的音频展示块，结束时原位替换为可下载的载体。
    part: Option<usize>,
    /// 转录始终追加到同一个独立展示块，不重复生成正文。
    transcript: Option<usize>,
}

impl Audio {
    /// 仅消费经 IR decoder 校验的两种音频事件，不展示其他原生扩展。
    pub(super) fn push(
        &mut self,
        event: &stream::Event,
        reply: &mut ChatReply,
    ) -> Result<(), String> {
        match event {
            stream::Event::Chat(chunk) => {
                for choice in chunk.choices.iter().filter(|choice| choice.index == 0) {
                    if let Some(audio) = choice.delta.audio.as_option() {
                        if let Some(data) = audio.data.as_option() {
                            self.append(data, reply)?;
                        }
                        if let Some(text) = audio.transcript.as_option() {
                            self.transcribe(text, reply);
                        }
                        if audio.expires_at.as_option().is_some() {
                            self.complete(reply);
                        }
                    }
                }
            }
            stream::Event::Responses(event) => {
                if let Event::Known(event) = event.as_ref() {
                    match event.as_ref() {
                        KnownEvent::AudioDelta(delta) => self.append(&delta.delta, reply)?,
                        KnownEvent::AudioTranscriptDelta(delta) => {
                            self.transcribe(&delta.delta, reply)
                        }
                        KnownEvent::AudioDone(_) => self.complete(reply),
                        // 转录 done 不携带正文，也不代表音频或整个响应结束。
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// 未完成的字节按最终 Base64 展示长度计入整个回复预算。
    pub(super) fn buffered_bytes(&self) -> usize {
        self.bytes.len().div_ceil(3) * 4
    }

    /// 每段独立解码后拼接；在扩容前限制最终编码体积。
    fn append(&mut self, data: &str, reply: &mut ChatReply) -> Result<(), String> {
        let bytes = STANDARD.decode(data).map_err(|_| "上游音频数据无效")?;
        if bytes.is_empty() {
            return Ok(());
        }
        if self.bytes.len().saturating_add(bytes.len()).div_ceil(3) * 4 > MAX_REPLY_BYTES {
            return Err("上游回复过长".into());
        }
        self.bytes.extend_from_slice(&bytes);
        self.part.get_or_insert_with(|| {
            let index = reply.parts.len();
            reply.parts.push(DisplayPart {
                title: "音频".into(),
                text: "正在接收音频…".into(),
                media: None,
            });
            index
        });
        Ok(())
    }

    /// 转录增量原位追加，只更新展示区，保留纯音频回复的空文本历史。
    fn transcribe(&mut self, text: &str, reply: &mut ChatReply) {
        if text.is_empty() {
            return;
        }
        let index = *self.transcript.get_or_insert_with(|| {
            let index = reply.parts.len();
            reply.parts.push(DisplayPart {
                title: "音频转录".into(),
                text: String::new(),
                media: None,
            });
            index
        });
        reply.parts[index].text.push_str(text);
    }

    /// 结束后仅编码一次并释放收集缓冲；格式依据容器头，不猜裸 PCM 参数。
    fn complete(&mut self, reply: &mut ChatReply) {
        let Some(index) = self.part else { return };
        let bytes = std::mem::take(&mut self.bytes);
        let mime = if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WAVE") {
            "audio/wav"
        } else if bytes.starts_with(b"OggS") {
            "audio/ogg"
        } else if bytes.starts_with(b"fLaC") {
            "audio/flac"
        } else if bytes.starts_with(b"ID3") {
            "audio/mpeg"
        } else {
            "application/octet-stream"
        };
        let preview = mime != "application/octet-stream";
        reply.parts[index] = DisplayPart {
            title: if preview {
                "音频"
            } else {
                "音频（格式未报告）"
            }
            .into(),
            text: if preview {
                ""
            } else {
                "下载音频；Provider 未报告编码格式"
            }
            .into(),
            media: Some(DisplayMedia {
                kind: MediaKind::Audio,
                uri: format!("data:{mime};base64,{}", STANDARD.encode(bytes)),
                preview,
            }),
        };
    }
}
