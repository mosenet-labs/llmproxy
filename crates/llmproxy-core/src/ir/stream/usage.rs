//! Provider 的流式用量是累计更新，不是可以相加的增量。
use super::super::usage::Usage;

/// 合并尚未完整报告的累计统计，保留缺失与明确报告零的区别。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UsageState {
    usage: Option<Usage>,
}

impl UsageState {
    /// 读取上游实际报告并合并后的用量；未报告任何用量时为空。
    pub fn snapshot(&self) -> Option<&Usage> {
        self.usage.as_ref()
    }

    /// 有值的累计字段覆盖旧值；缺失字段不清除已有报告。
    /// 输入或输出变更而合计未更新时，旧合计失效，不能展示过时数字。
    pub fn update(&mut self, update: &Usage) {
        let current = self.usage.get_or_insert_with(Usage::default);
        let changed = update
            .input_tokens
            .is_some_and(|n| Some(n) != current.input_tokens)
            || update
                .output_tokens
                .is_some_and(|n| Some(n) != current.output_tokens);
        if changed && update.total_tokens.is_none() {
            current.total_tokens = None;
        }
        macro_rules! copy {
            ($old:expr, $new:expr; $($field:ident),+ $(,)?) => {
                $(if $new.$field.is_some() { $old.$field = $new.$field; })+
            };
        }
        copy!(current, update; input_tokens, output_tokens, total_tokens);
        copy!(current.cache, update.cache; read_input_tokens, write_input_tokens, write_short_input_tokens, write_long_input_tokens);
        copy!(current.cache.read_details, update.cache.read_details; text_tokens, audio_tokens, image_tokens, video_tokens, document_tokens);
        copy!(current.input_details, update.input_details; uncached_tokens, text_tokens, audio_tokens, image_tokens, video_tokens, document_tokens, tool_tokens);
        copy!(current.input_details.tool_details, update.input_details.tool_details; text_tokens, audio_tokens, image_tokens, video_tokens, document_tokens);
        copy!(current.output_details, update.output_details; text_tokens, audio_tokens, image_tokens, video_tokens, reasoning_tokens, accepted_prediction_tokens, rejected_prediction_tokens);
    }
}
