//! Chat 分片中未进入语义 IR 的字段；音频原生字段由音频策略单独处理。
use crate::{
    adapter::protocol_codec::projection::{self, Notes},
    protocol::chat::response::Chunk,
};

/// 扩展键和元数据按字段报告，不输出对数概率或审核详情。
pub(super) fn collect(body: &Chunk, notes: &mut Notes) {
    notes.extra(&body.extra, "response");
    notes.field(&body.moderation, "response.moderation");
    notes.field(&body.obfuscation, "response.obfuscation");
    notes.field(&body.service_tier, "response.service_tier");
    notes.field(&body.system_fingerprint, "response.system_fingerprint");
    projection::chat_usage(notes, body.usage.as_option());
    for (i, choice) in body.choices.iter().enumerate() {
        let path = format!("choices[{i}]");
        notes.extra(&choice.extra, &path);
        notes.field(&choice.logprobs, &format!("{path}.logprobs"));
        let path = format!("{path}.delta");
        notes.object_extra(&choice.delta.extra, &["reasoning_content"], &path);
        if let Some(audio) = choice.delta.audio.as_option() {
            notes.extra(&audio.extra, &format!("{path}.audio"));
        }
        if let Some(call) = choice.delta.function_call.as_option() {
            notes.extra(&call.extra, &format!("{path}.function_call"));
        }
        for (i, call) in choice
            .delta
            .tool_calls
            .as_option()
            .into_iter()
            .flatten()
            .enumerate()
        {
            let path = format!("{path}.tool_calls[{i}]");
            notes.extra(&call.extra, &path);
            if let Some(function) = call.function.as_option() {
                notes.extra(&function.extra, &format!("{path}.function"));
            }
        }
    }
}
