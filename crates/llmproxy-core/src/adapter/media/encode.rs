use crate::{
    adapter::{Error, Result},
    ir::media::{Media, MediaKind as K, MediaSource as S, OriginalMedia as Raw},
    protocol::{
        OptionalNullable as O, Protocol, chat::request as c, gemini::request as g,
        messages::request as m, responses::request as r,
    },
};

/// 同协议未编辑媒体复用类型副本；其余情况仅凭通用字段构造目标媒体。
pub(crate) fn encode(media: &Media, target: Protocol) -> Result<Raw> {
    if let Some(original) = &media.original {
        let protocol = match original.as_ref() {
            Raw::Chat(_) => Protocol::OpenAiChat,
            Raw::Responses(_) => Protocol::OpenAiResponses,
            Raw::Messages(_) => Protocol::AnthropicMessages,
            Raw::Gemini(_) => Protocol::Gemini,
        };
        if protocol == target
            && let Some(before) = super::decode(original)
            && before == *media
        {
            return Ok(*original.clone());
        }
        if protocol == target && !super::metadata::extra(original)?.is_empty() {
            return Err(error("媒体含签名或其他专有附加字段，暂不能无损编辑"));
        }
    }
    if matches!(media.source, S::FileId(_)) {
        return Err(error(
            "Provider 文件 ID 不能跨协议复用，请提供公开 URL 或内联数据",
        ));
    }
    if let S::Url { uri, .. } = &media.source
        && !(uri.starts_with("https://") || uri.starts_with("http://"))
    {
        return Err(error("媒体地址不是可移植的 HTTP URL"));
    }
    if matches!(&media.source, S::Url { uri, .. } if uri.contains("generativelanguage.googleapis.com/") && uri.contains("/files/"))
    {
        return Err(error(
            "Gemini Files API 地址需要来源 Provider 鉴权，不能作为公开 URL 转发",
        ));
    }
    if matches!(media.source, S::Text(_)) && media.kind != K::File {
        return Err(error("纯文本文档不能作为图片、音频或视频"));
    }
    match target {
        Protocol::OpenAiChat => Ok(Raw::Chat(match media.kind {
            K::Image => c::UserPart::ImageUrl {
                image_url: c::ImageUrl {
                    url: as_url(&media.source)?,
                    detail: media.detail.clone(),
                    extra: Default::default(),
                },
                prompt_cache_breakpoint: O::Missing,
                extra: Default::default(),
            },
            K::Audio => {
                let S::Base64 { data, mime_type } = &media.source else {
                    return Err(error("Chat 音频需要内联 Base64 数据"));
                };
                let format = match mime_type.as_deref() {
                    Some("audio/wav" | "audio/x-wav") => "wav",
                    Some("audio/mp3" | "audio/mpeg") => "mp3",
                    _ => return Err(error("Chat input_audio 只支持 wav/mp3")),
                };
                c::UserPart::InputAudio {
                    input_audio: c::InputAudio {
                        data: data.clone(),
                        format: format.into(),
                        extra: Default::default(),
                    },
                    prompt_cache_breakpoint: O::Missing,
                    extra: Default::default(),
                }
            }
            K::File => {
                let S::Base64 { data, mime_type } = &media.source else {
                    return Err(error("Chat 文件需要内联数据或同 Provider 文件 ID"));
                };
                c::UserPart::File {
                    file: c::FileInput {
                        file_data: Some(data.clone()),
                        file_id: None,
                        filename: media.name.clone().or_else(|| {
                            (mime_type.as_deref() == Some("application/pdf"))
                                .then(|| "document.pdf".into())
                        }),
                        extra: Default::default(),
                    },
                    prompt_cache_breakpoint: O::Missing,
                    extra: Default::default(),
                }
            }
            K::Video => return Err(error("Chat 请求没有原生视频内容块")),
        })),
        Protocol::OpenAiResponses => Ok(Raw::Responses(match media.kind {
            K::Image => r::InputPart::InputImage {
                image_url: O::Value(as_url(&media.source)?),
                file_id: O::Missing,
                detail: media.detail.clone(),
                extra: Default::default(),
            },
            K::File if matches!(media.source, S::Text(_)) => {
                return Err(error(
                    "Responses 文件需要 URL 或 Base64 数据，不能直接使用文本文档来源",
                ));
            }
            K::File => r::InputPart::InputFile {
                file_data: if matches!(media.source, S::Base64 { .. }) {
                    Some(as_url(&media.source)?)
                } else {
                    None
                },
                file_url: if let S::Url { uri, .. } = &media.source {
                    Some(uri.clone())
                } else {
                    None
                },
                file_id: O::Missing,
                filename: media.name.clone(),
                detail: media.detail.clone(),
                extra: Default::default(),
            },
            _ => return Err(error("Responses 请求不支持此原生媒体类别")),
        })),
        Protocol::AnthropicMessages => {
            let source = match &media.source {
                S::Url { uri, .. } => serde_json::json!({"type":"url","url":uri}),
                S::Base64 { data, mime_type } => {
                    let mime = mime_type
                        .as_deref()
                        .ok_or_else(|| error("Messages 内联媒体需要 MIME 类型"))?;
                    if media.kind == K::Image
                        && !matches!(
                            mime,
                            "image/jpeg" | "image/png" | "image/gif" | "image/webp"
                        )
                    {
                        return Err(error("Messages 不支持此图片 MIME 类型"));
                    }
                    if media.kind == K::File && mime != "application/pdf" {
                        return Err(error(
                            "Messages Base64 文档仅支持 PDF，请使用 text source 传文本",
                        ));
                    }
                    serde_json::json!({"type":"base64","data":data,"media_type":mime})
                }
                S::Text(text) => {
                    serde_json::json!({"type":"text","data":text,"media_type":"text/plain"})
                }
                S::FileId(_) => unreachable!(),
            };
            Ok(Raw::Messages(m::ContentBlock::Known(match media.kind {
                K::Image => m::KnownContentBlock::Image {
                    source,
                    transformations: O::Missing,
                    cache_control: O::Missing,
                    extra: Default::default(),
                },
                K::File => m::KnownContentBlock::Document {
                    source,
                    cache_control: O::Missing,
                    citations: O::Missing,
                    context: O::Missing,
                    title: media.name.clone().into(),
                    extra: Default::default(),
                },
                _ => return Err(error("Messages 不支持原生音频／视频输入")),
            })))
        }
        Protocol::Gemini => {
            let mut part = g::Part::default();
            match &media.source {
                S::Url { uri, mime_type } => {
                    part.file_data = O::Value(g::FileData {
                        file_uri: uri.clone(),
                        mime_type: mime_type.clone().into(),
                        display_name: media.name.clone().into(),
                        extra: Default::default(),
                    })
                }
                S::Base64 { data, mime_type } => {
                    part.inline_data = O::Value(g::Blob {
                        data: data.clone(),
                        mime_type: mime_type
                            .clone()
                            .ok_or_else(|| error("Gemini 内联媒体需要 MIME 类型"))?,
                        display_name: media.name.clone().into(),
                        extra: Default::default(),
                    })
                }
                S::Text(text) => part.text = O::Value(text.clone()),
                S::FileId(_) => unreachable!(),
            }
            Ok(Raw::Gemini(Box::new(part)))
        }
    }
}
/// URL 与 Base64 Data URL 的转换仅操作已提供的数据。
fn as_url(source: &S) -> Result<String> {
    match source {
        S::Url { uri, .. } => Ok(uri.clone()),
        S::Base64 { data, mime_type } => Ok(format!(
            "data:{};base64,{data}",
            mime_type
                .as_deref()
                .ok_or_else(|| error("Data URL 缺少 MIME 类型"))?
        )),
        _ => Err(error("媒体来源不能表达为 URL")),
    }
}
fn error(message: &str) -> Error {
    Error::Unsupported(format!("media: {message}"))
}
