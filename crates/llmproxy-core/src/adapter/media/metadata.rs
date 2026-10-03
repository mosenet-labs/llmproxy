//! 尚未归一化的媒体附加字段，供同协议保护和跨协议警告使用。
use crate::{
    adapter::{Result, wire},
    ir::media::OriginalMedia as Raw,
    protocol::{
        OptionalNullable as O,
        chat::request::UserPart as C,
        messages::request::{ContentBlock as M, KnownContentBlock as B},
        responses::request::InputPart as R,
    },
};
use serde_json::{Map, Value};

/// 只提取附加字段，不把 URL、Base64 或整条消息复制到诊断元数据。
pub(super) fn extra(raw: &Raw) -> Result<Map<String, Value>> {
    let mut extra = match raw {
        Raw::Chat(C::ImageUrl {
            image_url,
            prompt_cache_breakpoint,
            extra,
        }) => {
            let mut extra = extra.clone();
            wire::put(
                &mut extra,
                "prompt_cache_breakpoint",
                prompt_cache_breakpoint,
            )?;
            if !image_url.extra.is_empty() {
                extra.insert("image_url".into(), Value::Object(image_url.extra.clone()));
            }
            extra
        }
        Raw::Chat(C::InputAudio {
            input_audio,
            prompt_cache_breakpoint,
            extra,
        }) => {
            let mut extra = extra.clone();
            wire::put(
                &mut extra,
                "prompt_cache_breakpoint",
                prompt_cache_breakpoint,
            )?;
            if !input_audio.extra.is_empty() {
                extra.insert(
                    "input_audio".into(),
                    Value::Object(input_audio.extra.clone()),
                );
            }
            extra
        }
        Raw::Chat(C::File {
            file,
            prompt_cache_breakpoint,
            extra,
        }) => {
            let mut extra = extra.clone();
            wire::put(
                &mut extra,
                "prompt_cache_breakpoint",
                prompt_cache_breakpoint,
            )?;
            if !file.extra.is_empty() {
                extra.insert("file".into(), Value::Object(file.extra.clone()));
            }
            extra
        }
        Raw::Responses(R::InputImage { extra, .. } | R::InputFile { extra, .. }) => extra.clone(),
        Raw::Messages(M::Known(B::Image {
            cache_control,
            transformations,
            extra,
            ..
        })) => {
            let mut extra = extra.clone();
            wire::put(&mut extra, "cache_control", cache_control)?;
            wire::put(&mut extra, "transformations", transformations)?;
            extra
        }
        Raw::Messages(M::Known(B::Document {
            cache_control,
            citations,
            context,
            extra,
            ..
        })) => {
            let mut extra = extra.clone();
            wire::put(&mut extra, "cache_control", cache_control)?;
            wire::put(&mut extra, "citations", citations)?;
            wire::put(&mut extra, "context", context)?;
            extra
        }
        Raw::Gemini(part) => {
            let mut extra = part.extra.clone();
            let nested = part
                .inline_data
                .as_option()
                .map(|v| &v.extra)
                .or_else(|| part.file_data.as_option().map(|v| &v.extra));
            if let Some(nested) = nested.filter(|v| !v.is_empty()) {
                extra.insert("media".into(), Value::Object(nested.clone()));
            }
            // 媒体之外还有其他有效载荷时，不能静默扔掉。
            let mut remainder = part.as_ref().clone();
            remainder.inline_data = O::Missing;
            remainder.file_data = O::Missing;
            for (key, value) in serde_json::to_value(remainder)?
                .as_object()
                .into_iter()
                .flatten()
            {
                extra.insert(key.clone(), value.clone());
            }
            extra
        }
        _ => Map::new(),
    };
    if let Raw::Messages(M::Known(B::Image { source, .. } | B::Document { source, .. })) = raw
        && let Some(source) = source.as_object()
    {
        let mut remainder = source.clone();
        for key in ["type", "data", "media_type", "url", "file_id"] {
            remainder.remove(key);
        }
        if !remainder.is_empty() {
            extra.insert("source".into(), Value::Object(remainder));
        }
    }
    Ok(extra)
}
