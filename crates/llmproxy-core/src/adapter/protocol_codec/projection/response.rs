//! 响应外壳、状态、候选和诊断的直接类型投影。
use super::Notes;
use crate::{
    ir::response::{Candidate, Failure, FinishReason, Response, Status},
    protocol::{Response as Source, responses::response::body::OutputItem},
};

/// 从协议结构体读取响应信息，不解释整包 JSON。
pub(in crate::adapter::protocol_codec) fn decode(response: &mut Response, source: &Source) {
    let mut notes = Notes::default();
    match source {
        Source::Chat(body) => {
            response.model = Some(body.model.clone());
            response.id = Some(body.id.clone());
            response.created_at = Some(body.created);
            response.status = Status::Completed;
            response.candidates = body
                .choices
                .iter()
                .enumerate()
                .map(|(i, c)| Candidate {
                    index: c.index,
                    items: vec![i],
                    finish_reason: finish(Some(&c.finish_reason)),
                })
                .collect();
            if body.object != "chat.completion" {
                notes.reject("object", "不是 Chat 完成响应");
            }
            for (i, c) in body.choices.iter().enumerate() {
                // Chat 音频响应不报告请求指定的编码格式，不能猜 MIME 类型生成其他协议媒体。
                // 参考：https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create
                if c.message.audio.as_option().is_some() {
                    notes.reject(
                        &format!("choices[{i}].message.audio"),
                        "音频输出缺少可移植格式上下文，不能跨协议转换",
                    );
                }
                notes.extra(&c.extra, &format!("choices[{i}]"));
                notes.field(&c.logprobs, &format!("choices[{i}].logprobs"));
            }
            notes.field(&body.metadata, "response.metadata");
            notes.field(&body.moderation, "response.moderation");
            notes.field(&body.service_tier, "response.service_tier");
            notes.field(&body.system_fingerprint, "response.system_fingerprint");
            notes.extra(&body.extra, "response");
            if let Some(usage) = body.usage.as_option() {
                notes.extra(&usage.extra, "usage");
            }
        }
        Source::Responses(body) => {
            response.model = Some(body.model.clone());
            response.id = Some(body.id.clone());
            response.created_at = Some(body.created_at);
            response.status = match body.status.as_option().map(String::as_str) {
                Some("completed") => Status::Completed,
                Some("incomplete") => Status::Incomplete,
                Some("failed") => Status::Failed,
                Some("cancelled") => Status::Cancelled,
                Some("in_progress" | "queued") => Status::InProgress,
                _ => Status::Unknown,
            };
            response.candidates = vec![Candidate {
                index: 0,
                items: (0..response.items.len()).collect(),
                finish_reason: match body
                    .incomplete_details
                    .as_option()
                    .map(|d| d.reason.as_str())
                {
                    Some("max_output_tokens") => FinishReason::Length,
                    Some("content_filter") => FinishReason::Filtered,
                    _ if response.status == Status::Completed => FinishReason::Stop,
                    Some("refusal") => FinishReason::Refusal,
                    _ => FinishReason::Unknown,
                },
            }];
            if body.object != "response" {
                notes.reject("object", "不是 Responses 完成响应");
            }
            for (i, item) in body.output.iter().enumerate() {
                if let OutputItem::FunctionCall(call) = item {
                    // 截断的函数参数仍可在使用字符串参数的协议之间原样表达。
                    if !(response.status == Status::Incomplete
                        && call.status.as_option().is_some_and(|s| s == "incomplete"))
                    {
                        super::request::check_status(
                            &mut notes,
                            &call.status,
                            &format!("output[{i}].status"),
                        );
                    }
                    notes.extra(&call.extra, &format!("output[{i}]"));
                }
            }
            response.failure = body.error.as_option().map(|error| Failure {
                code: error.code.clone(),
                message: error.message.clone(),
            });
            if let Some(error) = body.error.as_option() {
                notes.field(&error.misalignment, "response.error.misalignment");
                notes.extra(&error.extra, "response.error");
            }
            if response.failure.is_some() && response.status != Status::Failed {
                notes.reject("response.error", "非失败状态同时包含生成错误");
            }
            if let Some(details) = body.incomplete_details.as_option() {
                notes.extra(&details.extra, "response.incomplete_details");
            }
            notes.field(&body.instructions, "response.instructions");
            notes.field(&body.metadata, "response.metadata");
            notes.field(&body.parallel_tool_calls, "response.parallel_tool_calls");
            notes.field(&body.temperature, "response.temperature");
            notes.field(&body.tool_choice, "response.tool_choice");
            notes.field(&body.tools, "response.tools");
            notes.field(&body.top_p, "response.top_p");
            notes.field(&body.background, "response.background");
            notes.field(&body.completed_at, "response.completed_at");
            notes.field(&body.conversation, "response.conversation");
            notes.field(&body.max_output_tokens, "response.max_output_tokens");
            notes.field(&body.max_tool_calls, "response.max_tool_calls");
            notes.field(&body.moderation, "response.moderation");
            notes.field(&body.previous_response_id, "response.previous_response_id");
            notes.field(&body.prompt, "response.prompt");
            notes.field(
                &body.prompt_cache_diagnostics,
                "response.prompt_cache_diagnostics",
            );
            notes.field(&body.prompt_cache_key, "response.prompt_cache_key");
            notes.field(&body.prompt_cache_options, "response.prompt_cache_options");
            notes.field(
                &body.prompt_cache_retention,
                "response.prompt_cache_retention",
            );
            notes.field(&body.reasoning, "response.reasoning");
            notes.field(&body.service_tier, "response.service_tier");
            notes.field(&body.text, "response.text");
            notes.field(&body.truncation, "response.truncation");
            notes.extra(&body.extra, "response");
            if let Some(usage) = body.usage.as_option() {
                notes.extra(&usage.extra, "usage");
            }
        }
        Source::Messages(body) => {
            response.model = Some(body.model.clone());
            response.id = Some(body.id.clone());
            response.created_at = None;
            response.status = Status::Completed;
            response.candidates = vec![Candidate {
                index: 0,
                items: (0..response.items.len()).collect(),
                finish_reason: finish(body.stop_reason.as_option().map(String::as_str)),
            }];
            notes.field(&body.container, "response.container");
            notes.field(&body.diagnostics, "response.diagnostics");
            notes.field(&body.stop_details, "response.stop_details");
            notes.field(&body.stop_sequence, "response.stop_sequence");
            notes.extra(&body.extra, "response");
            let usage = &body.usage;
            notes.field(&usage.inference_geo, "usage.inference_geo");
            notes.field(&usage.server_tool_use, "usage.server_tool_use");
            notes.field(&usage.service_tier, "usage.service_tier");
            notes.extra(&usage.extra, "usage");
        }
        Source::Gemini(body) => {
            response.model = body.model_version.as_option().cloned();
            response.id = body.response_id.as_option().cloned();
            response.created_at = None;
            response.status = Status::Completed;
            let mut index = 0;
            response.candidates = body
                .candidates
                .as_option()
                .into_iter()
                .flatten()
                .enumerate()
                .map(|(position, c)| {
                    let items = if c.content.as_option().is_some() {
                        let i = index;
                        index += 1;
                        vec![i]
                    } else {
                        vec![]
                    };
                    Candidate {
                        index: c.index.as_option().copied().unwrap_or(position as u64),
                        items,
                        finish_reason: finish(c.finish_reason.as_option().map(String::as_str)),
                    }
                })
                .collect();
            for (i, c) in body
                .candidates
                .as_option()
                .into_iter()
                .flatten()
                .enumerate()
            {
                notes.field(&c.safety_ratings, &format!("candidates[{i}].safetyRatings"));
                notes.field(
                    &c.citation_metadata,
                    &format!("candidates[{i}].citationMetadata"),
                );
                notes.field(&c.token_count, &format!("candidates[{i}].tokenCount"));
                notes.field(
                    &c.grounding_attributions,
                    &format!("candidates[{i}].groundingAttributions"),
                );
                notes.field(
                    &c.grounding_metadata,
                    &format!("candidates[{i}].groundingMetadata"),
                );
                notes.field(&c.avg_logprobs, &format!("candidates[{i}].avgLogprobs"));
                notes.field(
                    &c.logprobs_result,
                    &format!("candidates[{i}].logprobsResult"),
                );
                notes.field(
                    &c.url_context_metadata,
                    &format!("candidates[{i}].urlContextMetadata"),
                );
                notes.field(&c.finish_message, &format!("candidates[{i}].finishMessage"));
                notes.extra(&c.extra, &format!("candidates[{i}]"));
            }
            if response.candidates.is_empty()
                && body.prompt_feedback.as_option().is_some_and(|feedback| {
                    feedback
                        .block_reason
                        .as_option()
                        .is_some_and(|reason| reason != "BLOCK_REASON_UNSPECIFIED")
                })
            {
                // 请求在生成前被过滤时没有候选，但过滤状态仍需返回给客户端。
                response.candidates.push(Candidate {
                    index: 0,
                    items: vec![],
                    finish_reason: FinishReason::Filtered,
                });
            }
            notes.field(&body.prompt_feedback, "response.promptFeedback");
            notes.field(&body.model_status, "response.modelStatus");
            notes.extra(&body.extra, "response");
            if let Some(usage) = body.usage_metadata.as_option() {
                for (details, path) in [
                    (&usage.prompt_tokens_details, "usage.promptTokensDetails"),
                    (&usage.cache_tokens_details, "usage.cacheTokensDetails"),
                    (
                        &usage.candidates_tokens_details,
                        "usage.candidatesTokensDetails",
                    ),
                    (
                        &usage.tool_use_prompt_tokens_details,
                        "usage.toolUsePromptTokensDetails",
                    ),
                ] {
                    if let Some(details) = details.as_option() {
                        let mut totals = std::collections::HashMap::<String, u64>::new();
                        for (i, detail) in details.iter().enumerate() {
                            let modality = detail.modality.to_ascii_uppercase();
                            if !matches!(
                                modality.as_str(),
                                "TEXT" | "AUDIO" | "IMAGE" | "VIDEO" | "DOCUMENT"
                            ) {
                                notes.dropped(format!("{path}[{i}].modality"));
                            }
                            let total = totals.entry(modality).or_default();
                            if let Some(sum) = total.checked_add(detail.token_count) {
                                *total = sum;
                            } else {
                                notes.reject(path, "模态词元数溢出");
                            }
                            notes.extra(&detail.extra, &format!("{path}[{i}]"));
                        }
                    }
                }
                notes.field(&usage.service_tier, "usage.serviceTier");
                notes.extra(&usage.extra, "usage");
            }
        }
    }
    response.diagnostics = notes.0;
}
/// 未识别结束原因保持 Unknown，不伪造成功。
fn finish(reason: Option<&str>) -> FinishReason {
    match reason {
        Some("stop" | "end_turn" | "stop_sequence" | "STOP") => FinishReason::Stop,
        Some("tool_calls" | "tool_use" | "function_call") => FinishReason::ToolCall,
        Some("length" | "max_tokens" | "MAX_TOKENS") => FinishReason::Length,
        Some(
            "content_filter"
            | "SAFETY"
            | "BLOCKLIST"
            | "PROHIBITED_CONTENT"
            | "SPII"
            | "RECITATION"
            | "IMAGE_SAFETY"
            | "IMAGE_PROHIBITED_CONTENT",
        ) => FinishReason::Filtered,
        Some("refusal") => FinishReason::Refusal,
        _ => FinishReason::Unknown,
    }
}
