//! Gemini 的生成元数据与正文独立处理，媒体叶子的来源副本由媒体适配器维护。
use crate::{
    adapter::protocol_codec::projection::{self, Notes},
    protocol::gemini::response::Response,
};

/// 文本、工具或执行结果上的附属字段不会静默消失。
pub(super) fn collect(body: &Response, notes: &mut Notes) {
    notes.extra(&body.extra, "response");
    notes.field(&body.prompt_feedback, "response.promptFeedback");
    notes.field(&body.model_status, "response.modelStatus");
    projection::gemini_usage(notes, body.usage_metadata.as_option());
    for (i, candidate) in body
        .candidates
        .as_option()
        .into_iter()
        .flatten()
        .enumerate()
    {
        let path = format!("candidates[{i}]");
        notes.extra(&candidate.extra, &path);
        macro_rules! field { ($($name:ident => $wire:literal),+ $(,)?) => { $(notes.field(&candidate.$name, &format!("{path}.{}", $wire));)+ }; }
        field!(safety_ratings => "safetyRatings", citation_metadata => "citationMetadata", token_count => "tokenCount", grounding_attributions => "groundingAttributions", grounding_metadata => "groundingMetadata", avg_logprobs => "avgLogprobs", logprobs_result => "logprobsResult", url_context_metadata => "urlContextMetadata", finish_message => "finishMessage");
        if let Some(content) = candidate.content.as_option() {
            notes.extra(&content.extra, &format!("{path}.content"));
            for (i, part) in content.parts.iter().enumerate() {
                // 媒体块已有类型化来源副本，同协议编码可保留整块附属字段。
                if part.inline_data.as_option().is_some() || part.file_data.as_option().is_some() {
                    continue;
                }
                let path = format!("{path}.content.parts[{i}]");
                notes.extra(&part.extra, &path);
                macro_rules! field { ($($name:ident => $wire:literal),+ $(,)?) => { $(notes.field(&part.$name, &format!("{path}.{}", $wire));)+ }; }
                field!(part_metadata => "partMetadata", media_resolution => "mediaResolution", media_processing => "mediaProcessing", audio_transcription => "audioTranscription", speech_metadata => "speechMetadata", video_metadata => "videoMetadata");
                if let Some(call) = part.function_call.as_option() {
                    notes.extra(&call.extra, &format!("{path}.functionCall"));
                }
            }
        }
    }
}
