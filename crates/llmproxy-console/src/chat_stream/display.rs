//! 从响应 IR 提取页面内容；展示标题、统计和媒体链接不混入模型历史。
use super::ChatReply;
use llmproxy_core::ir::{
    media::{Media, MediaKind, MediaSource},
    message::{PartKind, ToolCall},
    response::{Item, Response},
    server_output::{Kind, ServerOutput},
};
use serde::{Deserialize, Serialize};

/// 文本正文之外的独立展示块，不保存签名或原始协议扩展。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct DisplayPart {
    /// 空标题表示普通正文，非空标题表示工具或附件说明。
    pub title: String,
    /// 可见正文或格式化后的工具参数/结果。
    pub text: String,
    /// 已筛选的媒体载体。
    pub media: Option<DisplayMedia>,
}

/// 已筛选的可展示媒体地址；缺失格式时仅提供下载。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct DisplayMedia {
    /// 用于选择图片、音频、视频或下载控件。
    pub kind: MediaKind,
    /// 可公开访问的地址或经过类型筛选的内联数据。
    pub uri: String,
    /// 是否具备明确且可被浏览器预览的 MIME 类型。
    pub preview: bool,
}

/// 只读取序号最小的候选，保持该候选内部输出项和片段的顺序。
pub(super) fn read(ir: &Response, reply: &mut ChatReply) {
    let indices = ir
        .candidates
        .iter()
        .min_by_key(|c| c.index)
        .map(|c| c.items.clone())
        .unwrap_or_else(|| (0..ir.items.len()).collect());
    for index in indices {
        match ir.items.get(index) {
            Some(Item::Message(index)) => {
                if let Some(message) = ir.messages.get(*index) {
                    for part in &message.parts {
                        match &part.kind {
                            PartKind::Text(text) | PartKind::Refusal(text) => {
                                reply.content.push_str(text);
                                if !text.is_empty() {
                                    if let Some(previous) = reply.parts.last_mut().filter(|part| {
                                        part.title.is_empty() && part.media.is_none()
                                    }) {
                                        previous.text.push_str(text);
                                    } else {
                                        reply.parts.push(DisplayPart {
                                            title: String::new(),
                                            text: text.clone(),
                                            media: None,
                                        });
                                    }
                                }
                            }
                            PartKind::Reasoning(value) => {
                                if let Some(text) = value.as_str() {
                                    reply.thinking.push_str(text);
                                }
                            }
                            PartKind::ToolCall(call) => call_part(call, reply),
                            PartKind::ToolResult(result) => reply.parts.push(DisplayPart {
                                title: format!(
                                    "工具结果 · {}",
                                    result
                                        .name
                                        .as_deref()
                                        .or(result.id.as_deref())
                                        .unwrap_or("未命名工具")
                                ),
                                text: json_text(&result.content),
                                media: None,
                            }),
                            PartKind::ServerOutput(output) => server_part(output, reply),
                            PartKind::Media(media) => media_part(media, reply),
                            PartKind::Opaque(_) => {} // 不展示未知叶子，避免泄露签名和加密思考。
                        }
                    }
                }
            }
            Some(Item::ToolCall { call, .. }) => call_part(call, reply),
            Some(Item::Reasoning(text)) => reply.summary.push_str(text),
            Some(Item::ServerOutput(output)) => server_part(output, reply),
            Some(Item::Opaque(_)) | None => {}
        }
    }
}

/// 工具参数按 JSON 排版；完整字符串参数保留原文，不伪造执行结果。
fn call_part(call: &ToolCall, reply: &mut ChatReply) {
    reply.parts.push(DisplayPart {
        title: format!("工具调用 · {}", call.name),
        text: json_text(&call.arguments),
        media: None,
    });
}

/// 任意工具叶子展示为纯文本，避免被解释为页面标记。
fn json_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        _ => serde_json::to_string_pretty(value).expect("JSON Value 可序列化"),
    }
}

/// 服务端记录只展示已规范化的可见内容，不读取 original 中的私有数据。
fn server_part(output: &ServerOutput, reply: &mut ChatReply) {
    let title = match output.kind {
        Kind::Code => "服务端代码",
        Kind::ExecutionResult => "服务端执行结果",
        Kind::SearchResult => "搜索结果",
        Kind::Action => "服务端工具",
    };
    reply.parts.push(DisplayPart {
        title: title.into(),
        text: output
            .text
            .clone()
            .unwrap_or_else(|| "Provider 已执行此操作".into()),
        media: None,
    });
}

/// 媒体地址只允许公开 HTTP(S) 或内联 Base64，Provider 文件 ID 只显示不可取回说明。
fn media_part(media: &Media, reply: &mut ChatReply) {
    let title = media
        .name
        .clone()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| {
            match media.kind {
                MediaKind::Image => "图片",
                MediaKind::Audio => "音频",
                MediaKind::Video => "视频",
                MediaKind::File => "文件",
            }
            .into()
        });
    let (uri, mime) = match &media.source {
        MediaSource::Url { uri, mime_type } => {
            let uri = url::Url::parse(uri)
                .ok()
                .filter(|url| {
                    matches!(url.scheme(), "http" | "https")
                        && url.username().is_empty()
                        && url.password().is_none()
                })
                .map(|url| url.to_string());
            (uri, mime_type.as_deref())
        }
        MediaSource::Base64 { data, mime_type } => {
            let mime = mime_type
                .as_deref()
                .filter(|mime| safe_mime(mime))
                .unwrap_or("application/octet-stream");
            let uri = inline_uri(data, mime);
            (uri, mime_type.as_deref())
        }
        MediaSource::Text(text) => {
            reply.parts.push(DisplayPart {
                title,
                text: text.clone(),
                media: None,
            });
            return;
        }
        MediaSource::FileId(_) => (None, None),
    };
    let preview = mime.is_some_and(|mime| match media.kind {
        MediaKind::Image => matches!(
            mime,
            "image/png" | "image/jpeg" | "image/gif" | "image/webp"
        ),
        MediaKind::Audio => matches!(
            mime,
            "audio/wav" | "audio/mpeg" | "audio/ogg" | "audio/flac"
        ),
        MediaKind::Video => matches!(mime, "video/mp4" | "video/webm"),
        MediaKind::File => false,
    });
    let text = if uri.is_none() {
        "媒体未提供可公开访问的地址"
    } else if !preview {
        "下载附件（不支持预览或格式未报告）"
    } else {
        ""
    };
    reply.parts.push(DisplayPart {
        title,
        text: text.into(),
        media: uri.map(|uri| DisplayMedia {
            kind: media.kind,
            uri,
            preview,
        }),
    });
}

/// HTML/SVG 等主动内容不作为 data URI 的类型，下载使用二进制类型。
fn safe_mime(mime: &str) -> bool {
    matches!(
        mime,
        "image/png"
            | "image/jpeg"
            | "image/gif"
            | "image/webp"
            | "audio/wav"
            | "audio/mpeg"
            | "audio/ogg"
            | "audio/flac"
            | "video/mp4"
            | "video/webm"
            | "application/pdf"
            | "application/octet-stream"
    )
}

/// Chat 音频响应没有编码格式，沿用二进制下载，转录文字独立展示。
pub(super) fn chat_audio(
    audio: &llmproxy_core::protocol::chat::response::message::Audio,
    reply: &mut ChatReply,
) {
    reply.parts.push(DisplayPart {
        title: "音频（格式未报告）".into(),
        text: "下载音频；Provider 未报告编码格式".into(),
        media: inline_uri(&audio.data, "application/octet-stream").map(|uri| DisplayMedia {
            kind: MediaKind::Audio,
            uri,
            preview: false,
        }),
    });
    if !audio.transcript.is_empty() {
        reply.parts.push(DisplayPart {
            title: "音频转录".into(),
            text: audio.transcript.clone(),
            media: None,
        });
    }
}

/// 内联数据只允许 Base64 字符，地址头不接受上游提供的主动内容类型。
fn inline_uri(data: &str, mime: &str) -> Option<String> {
    data.bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='))
        .then(|| format!("data:{mime};base64,{data}"))
}
