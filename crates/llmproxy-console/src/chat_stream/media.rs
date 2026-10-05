//! 流式与非流式展示共用媒体类型策略和音频附件构造。
use super::display::{DisplayMedia, DisplayPart};
use base64::{Engine, engine::general_purpose::STANDARD};
use llmproxy_core::ir::media::MediaKind;

/// HTML/SVG 等主动内容只允许二进制下载，不作为内联类型。
pub(super) fn safe_mime(mime: &str) -> bool {
    preview(MediaKind::Image, mime)
        || preview(MediaKind::Audio, mime)
        || preview(MediaKind::Video, mime)
        || matches!(mime, "application/pdf" | "application/octet-stream")
}

/// 页面支持的预览类型只在这里声明，附件标题不参与类型判断。
pub(super) fn preview(kind: MediaKind, mime: &str) -> bool {
    match kind {
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
    }
}

/// 内联地址只使用允许的类型及 Base64 字符，不接收上游提供的地址头。
pub(super) fn inline_uri(data: &str, mime: &str) -> Option<String> {
    data.bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='))
        .then(|| format!("data:{mime};base64,{data}"))
}

/// 两种响应模式共用容器识别；未知格式只下载，不猜裸 PCM 的声道和采样率。
pub(super) fn audio_part(bytes: &[u8]) -> DisplayPart {
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
    let preview = preview(MediaKind::Audio, mime);
    DisplayPart {
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
    }
}
