use crate::{
    ir::media::{Media, MediaKind as K, MediaSource as S, OriginalMedia as Raw},
    protocol::{
        chat::request::UserPart as C,
        messages::request::{ContentBlock as M, KnownContentBlock as B},
        responses::request::InputPart as R,
    },
};

/// 从已知媒体类型提取来源；不合法或尚未支持的来源交给不透明处理策略。
pub(crate) fn decode(raw: &Raw) -> Option<Media> {
    let (kind, source, name, detail) = match raw {
        Raw::Chat(C::ImageUrl { image_url, .. }) => (
            K::Image,
            url(&image_url.url),
            None,
            image_url.detail.clone(),
        ),
        Raw::Chat(C::InputAudio { input_audio, .. }) => (
            K::Audio,
            S::Base64 {
                data: input_audio.data.clone(),
                mime_type: Some(format!("audio/{}", input_audio.format)),
            },
            None,
            None,
        ),
        Raw::Chat(C::File { file, .. }) => (
            K::File,
            if let Some(id) = &file.file_id {
                S::FileId(id.clone())
            } else {
                data(file.file_data.as_ref()?, file.filename.as_deref())
            },
            file.filename.clone(),
            None,
        ),
        Raw::Responses(R::InputImage {
            image_url,
            file_id,
            detail,
            ..
        }) => (
            K::Image,
            if let Some(id) = file_id.as_option() {
                S::FileId(id.clone())
            } else {
                url(image_url.as_option()?)
            },
            None,
            detail.clone(),
        ),
        Raw::Responses(R::InputFile {
            file_data,
            file_id,
            file_url,
            filename,
            detail,
            ..
        }) => (
            K::File,
            if let Some(id) = file_id.as_option() {
                S::FileId(id.clone())
            } else if let Some(url) = file_url {
                S::Url {
                    uri: url.clone(),
                    mime_type: None,
                }
            } else {
                data(file_data.as_ref()?, filename.as_deref())
            },
            filename.clone(),
            detail.clone(),
        ),
        Raw::Messages(M::Known(B::Image { source, .. })) => {
            (K::Image, message_source(source)?, None, None)
        }
        Raw::Messages(M::Known(B::Document { source, title, .. })) => (
            K::File,
            message_source(source)?,
            title.as_option().cloned(),
            None,
        ),
        Raw::Gemini(part) => {
            if let Some(blob) = part.inline_data.as_option() {
                (
                    kind(&blob.mime_type),
                    S::Base64 {
                        data: blob.data.clone(),
                        mime_type: Some(blob.mime_type.clone()),
                    },
                    blob.display_name.as_option().cloned(),
                    None,
                )
            } else {
                let file = part.file_data.as_option()?;
                (
                    kind(file.mime_type.as_option().map(String::as_str).unwrap_or("")),
                    S::Url {
                        uri: file.file_uri.clone(),
                        mime_type: file.mime_type.as_option().cloned(),
                    },
                    file.display_name.as_option().cloned(),
                    None,
                )
            }
        }
        _ => return None,
    };
    Some(Media {
        kind,
        source,
        name,
        detail,
        original: Some(Box::new(raw.clone())),
    })
}
/// Data URL 的 MIME 信息不可丢弃，其他地址保持 URL。
fn url(value: &str) -> S {
    if let Some((header, body)) = value.strip_prefix("data:").and_then(|v| v.split_once(','))
        && let Some(mime) = header.strip_suffix(";base64")
    {
        S::Base64 {
            data: body.into(),
            mime_type: Some(mime.into()),
        }
    } else {
        S::Url {
            uri: value.into(),
            mime_type: None,
        }
    }
}
/// 文件的裸 Base64 只能从明确的文件扩展名确定 MIME，不能默认当作 PDF。
fn data(value: &str, name: Option<&str>) -> S {
    if value.starts_with("data:") {
        return url(value);
    }
    S::Base64 {
        data: value.into(),
        mime_type: name
            .and_then(|n| n.rsplit('.').next())
            .and_then(|ext| match ext.to_ascii_lowercase().as_str() {
                "pdf" => Some("application/pdf"),
                "txt" => Some("text/plain"),
                "png" => Some("image/png"),
                "jpg" | "jpeg" => Some("image/jpeg"),
                _ => None,
            })
            .map(str::to_owned),
    }
}
/// MIME 首部决定媒体类别。
fn kind(mime: &str) -> K {
    if mime.starts_with("image/") {
        K::Image
    } else if mime.starts_with("audio/") {
        K::Audio
    } else if mime.starts_with("video/") {
        K::Video
    } else {
        K::File
    }
}
/// Messages 的开放 source 仅在媒体叶子处读取已知字段。
fn message_source(source: &serde_json::Value) -> Option<S> {
    let value = |key: &str| {
        source
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    };
    match source.get("type")?.as_str()? {
        "base64" => Some(S::Base64 {
            data: value("data")?,
            mime_type: value("media_type"),
        }),
        "url" => Some(S::Url {
            uri: value("url")?,
            mime_type: None,
        }),
        "file" => Some(S::FileId(value("file_id")?)),
        "text" => Some(S::Text(value("data")?)),
        _ => None,
    }
}
