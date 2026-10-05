//! Gemini `usageMetadata` 与通用词元统计的双向映射。

use crate::{
    adapter::nullable::{present, set},
    ir::{
        cache::CacheUsage,
        usage::{InputTokenDetails, ModalityTokenDetails, OutputTokenDetails, Usage as IrUsage},
    },
    protocol::{
        OptionalNullable,
        gemini::response::usage::{ModalityTokenCount, UsageMetadata},
    },
};

/// 提取输入、缓存读取、输出、思考及各模态计数。
pub fn decode_gemini_usage(usage: &UsageMetadata) -> IrUsage {
    let input = details(&usage.prompt_tokens_details);
    let output = details(&usage.candidates_tokens_details);
    let prompt = present(&usage.prompt_token_count);
    let cached = present(&usage.cached_content_token_count);
    let candidates = present(&usage.candidates_token_count);
    let thoughts = present(&usage.thoughts_token_count);
    // Gemini 将思考词元单列，但它仍属于总输出用量。
    let output_total = present(&usage.total_token_count)
        .zip(prompt)
        .and_then(|(total, prompt)| total.checked_sub(prompt))
        .or_else(|| candidates.and_then(|count| count.checked_add(thoughts.unwrap_or(0))));
    IrUsage {
        input_tokens: prompt,
        output_tokens: output_total,
        total_tokens: present(&usage.total_token_count),
        cache: CacheUsage {
            read_input_tokens: cached,
            read_details: decode_modalities(&usage.cache_tokens_details),
            ..Default::default()
        },
        input_details: InputTokenDetails {
            uncached_tokens: prompt
                .zip(cached)
                .and_then(|(total, cached)| total.checked_sub(cached)),
            text_tokens: modality_count(input, "TEXT"),
            audio_tokens: modality_count(input, "AUDIO"),
            image_tokens: modality_count(input, "IMAGE"),
            video_tokens: modality_count(input, "VIDEO"),
            document_tokens: modality_count(input, "DOCUMENT"),
            tool_tokens: present(&usage.tool_use_prompt_token_count),
            tool_details: decode_modalities(&usage.tool_use_prompt_tokens_details),
        },
        output_details: OutputTokenDetails {
            text_tokens: modality_count(output, "TEXT"),
            audio_tokens: modality_count(output, "AUDIO"),
            image_tokens: modality_count(output, "IMAGE"),
            video_tokens: modality_count(output, "VIDEO"),
            reasoning_tokens: thoughts,
            ..Default::default()
        },
    }
}

/// 将通用计数写回 Gemini；原始模态与新增字段保持不变。
pub fn encode_gemini_usage(usage: &IrUsage, original: Option<&UsageMetadata>) -> UsageMetadata {
    let mut encoded = original.cloned().unwrap_or_default();
    set(&mut encoded.prompt_token_count, usage.input_tokens);
    let thoughts = usage
        .output_details
        .reasoning_tokens
        .or_else(|| original.and_then(|value| present(&value.thoughts_token_count)))
        .unwrap_or(0);
    set(
        &mut encoded.candidates_token_count,
        usage
            .output_tokens
            .and_then(|total| total.checked_sub(thoughts)),
    );
    set(&mut encoded.total_token_count, usage.total_tokens);
    set(
        &mut encoded.cached_content_token_count,
        usage.cache.read_input_tokens,
    );
    set(
        &mut encoded.tool_use_prompt_token_count,
        usage.input_details.tool_tokens,
    );
    set(
        &mut encoded.thoughts_token_count,
        usage.output_details.reasoning_tokens,
    );
    encode_modalities(&mut encoded.cache_tokens_details, &usage.cache.read_details);
    encode_modalities(
        &mut encoded.tool_use_prompt_tokens_details,
        &usage.input_details.tool_details,
    );
    for (modality, count) in [
        ("TEXT", usage.input_details.text_tokens),
        ("AUDIO", usage.input_details.audio_tokens),
        ("IMAGE", usage.input_details.image_tokens),
        ("VIDEO", usage.input_details.video_tokens),
        ("DOCUMENT", usage.input_details.document_tokens),
    ] {
        set_modality(&mut encoded.prompt_tokens_details, modality, count);
    }
    for (modality, count) in [
        ("TEXT", usage.output_details.text_tokens),
        ("AUDIO", usage.output_details.audio_tokens),
        ("IMAGE", usage.output_details.image_tokens),
        ("VIDEO", usage.output_details.video_tokens),
    ] {
        set_modality(&mut encoded.candidates_tokens_details, modality, count);
    }
    encoded
}

/// 缓存和工具输入使用同一套模态计数，避免把细分计数再次计入总用量。
/// 参考：https://ai.google.dev/api/generate-content#UsageMetadata
fn decode_modalities(value: &OptionalNullable<Vec<ModalityTokenCount>>) -> ModalityTokenDetails {
    let items = details(value);
    ModalityTokenDetails {
        text_tokens: modality_count(items, "TEXT"),
        audio_tokens: modality_count(items, "AUDIO"),
        image_tokens: modality_count(items, "IMAGE"),
        video_tokens: modality_count(items, "VIDEO"),
        document_tokens: modality_count(items, "DOCUMENT"),
    }
}

/// 写回可移植的模态明细；未知模态仍由同协议来源副本保存。
fn encode_modalities(
    target: &mut OptionalNullable<Vec<ModalityTokenCount>>,
    value: &ModalityTokenDetails,
) {
    for (modality, count) in [
        ("TEXT", value.text_tokens),
        ("AUDIO", value.audio_tokens),
        ("IMAGE", value.image_tokens),
        ("VIDEO", value.video_tokens),
        ("DOCUMENT", value.document_tokens),
    ] {
        set_modality(target, modality, count);
    }
}

/// 取得可用的模态明细。
fn details(value: &OptionalNullable<Vec<ModalityTokenCount>>) -> &[ModalityTokenCount] {
    match value {
        OptionalNullable::Value(details) => details,
        _ => &[],
    }
}

/// 同一模态出现多次时累计；未报告时保持 `None`。
fn modality_count(details: &[ModalityTokenCount], modality: &str) -> Option<u64> {
    let mut count = None;
    for detail in details
        .iter()
        .filter(|detail| detail.modality.eq_ignore_ascii_case(modality))
    {
        count = Some(count.unwrap_or(0_u64).checked_add(detail.token_count)?);
    }
    count
}

/// 未编辑时保留重复项原貌；编辑时合并为一项，清空时删除该模态。
fn set_modality(
    target: &mut OptionalNullable<Vec<ModalityTokenCount>>,
    modality: &str,
    value: Option<u64>,
) {
    if modality_count(details(target), modality) == value {
        return;
    }
    let Some(value) = value else {
        if let OptionalNullable::Value(details) = target {
            details.retain(|detail| !detail.modality.eq_ignore_ascii_case(modality));
        }
        return;
    };
    if !matches!(target, OptionalNullable::Value(_)) {
        *target = OptionalNullable::Value(Vec::new());
    }
    if let OptionalNullable::Value(details) = target {
        if let Some(detail) = details
            .iter_mut()
            .find(|detail| detail.modality.eq_ignore_ascii_case(modality))
        {
            detail.token_count = value;
            let mut found = false;
            details.retain(|detail| {
                if !detail.modality.eq_ignore_ascii_case(modality) {
                    return true;
                }
                let keep = !found;
                found = true;
                keep
            });
        } else {
            details.push(ModalityTokenCount {
                modality: modality.into(),
                token_count: value,
                extra: Default::default(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{decode_gemini_usage, encode_gemini_usage};
    use crate::protocol::gemini::response::usage::UsageMetadata;
    use serde_json::json;

    #[test]
    fn usage_keeps_cache_and_modality_counts_separate() {
        let source = json!({"promptTokenCount":100,"cachedContentTokenCount":40,"candidatesTokenCount":20,"thoughtsTokenCount":5,"totalTokenCount":125,"promptTokensDetails":[{"modality":"TEXT","tokenCount":70},{"modality":"IMAGE","tokenCount":30}],"cacheTokensDetails":[{"modality":"TEXT","tokenCount":40}],"candidatesTokensDetails":[{"modality":"TEXT","tokenCount":20}],"vendor":true});
        let raw: UsageMetadata = serde_json::from_value(source.clone()).unwrap();
        let mut ir = decode_gemini_usage(&raw);
        assert_eq!(ir.input_tokens, Some(100));
        assert_eq!(ir.output_tokens, Some(25));
        assert_eq!(ir.cache.read_input_tokens, Some(40));
        assert_eq!(ir.input_details.uncached_tokens, Some(60));
        assert_eq!(ir.input_details.image_tokens, Some(30));
        assert_eq!(
            serde_json::to_value(encode_gemini_usage(&ir, Some(&raw))).unwrap(),
            source
        );
        ir.input_details.image_tokens = Some(35);
        let encoded = serde_json::to_value(encode_gemini_usage(&ir, Some(&raw))).unwrap();
        assert_eq!(encoded["promptTokensDetails"][1]["tokenCount"], 35);
        assert_eq!(encoded["cacheTokensDetails"][0]["tokenCount"], 40);
    }

    #[test]
    fn output_total_includes_thoughts_when_only_component_counts_exist() {
        let raw: UsageMetadata = serde_json::from_value(json!({
            "promptTokenCount": 100,
            "candidatesTokenCount": 20,
            "thoughtsTokenCount": 5
        }))
        .unwrap();
        let mut ir = decode_gemini_usage(&raw);
        assert_eq!(ir.output_tokens, Some(25));
        ir.output_tokens = Some(30);
        let encoded = encode_gemini_usage(&ir, Some(&raw));
        assert_eq!(
            encoded.candidates_token_count,
            crate::protocol::OptionalNullable::Value(25)
        );
    }

    #[test]
    fn cache_and_tool_modalities_can_be_rebuilt_edited_and_cleared() {
        let raw: UsageMetadata = serde_json::from_value(json!({
            "promptTokenCount":100,"cachedContentTokenCount":30,
            "toolUsePromptTokenCount":10,"candidatesTokenCount":5,"totalTokenCount":105,
            "cacheTokensDetails":[{"modality":"TEXT","tokenCount":10},{"modality":"text","tokenCount":20}],
            "toolUsePromptTokensDetails":[{"modality":"TEXT","tokenCount":0},{"modality":"IMAGE","tokenCount":10}]
        })).unwrap();
        let mut ir = decode_gemini_usage(&raw);
        assert_eq!(ir.cache.read_details.text_tokens, Some(30));
        assert_eq!(ir.input_details.tool_details.text_tokens, Some(0));
        assert_eq!(ir.input_details.tool_details.image_tokens, Some(10));
        assert_eq!(ir.input_tokens, Some(100));
        assert_eq!(ir.output_tokens, Some(5));
        assert_eq!(encode_gemini_usage(&ir, Some(&raw)), raw);
        assert_eq!(decode_gemini_usage(&encode_gemini_usage(&ir, None)), ir);
        ir.cache.read_details.text_tokens = Some(25);
        ir.input_details.tool_details.image_tokens = None;
        let edited = encode_gemini_usage(&ir, Some(&raw));
        assert_eq!(edited.cache_tokens_details.as_option().unwrap().len(), 1);
        assert_eq!(decode_gemini_usage(&edited), ir);
    }
}
