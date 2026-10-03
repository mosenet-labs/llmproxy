//! 请求字段直接由协议结构体投影。
use super::Notes;
use crate::{
    ir::request::{Generation, Request},
    protocol::{
        OptionalNullable as O, Request as Source,
        chat::request::parameters::StopSequences,
        messages::request::body::SystemPrompt,
        responses::request::body::{Input, InputItem},
    },
};

/// 提取模型、生成参数、客户端函数以及未映射字段诊断。
pub(in crate::adapter::protocol_codec) fn decode(request: &mut Request, source: &Source) {
    let mut notes = Notes::default();
    match source {
        Source::Chat(body) => {
            request.model = Some(body.model.clone());
            request.generation = Generation {
                max_output_tokens: body.max_completion_tokens.as_option().copied(),
                temperature: body.temperature.as_option().copied(),
                top_p: body.top_p.as_option().copied(),
                stop_sequences: body.stop.as_option().map(|stop| match stop {
                    StopSequences::One(s) => vec![s.clone()],
                    StopSequences::Many(s) => s.clone(),
                }),
                candidate_count: body.n.as_option().copied(),
                stream: body.stream.as_option().copied().unwrap_or(false),
            };
            notes.field(&body.audio, "request.audio");
            notes.field(&body.frequency_penalty, "request.frequency_penalty");
            notes.field(&body.function_call, "request.function_call");
            notes.field(&body.functions, "request.functions");
            notes.field(&body.logit_bias, "request.logit_bias");
            notes.field(&body.logprobs, "request.logprobs");
            notes.field(&body.max_tokens, "request.max_tokens");
            notes.field(&body.metadata, "request.metadata");
            notes.field(&body.modalities, "request.modalities");
            notes.field(&body.moderation, "request.moderation");
            notes.field(&body.parallel_tool_calls, "request.parallel_tool_calls");
            notes.field(&body.prediction, "request.prediction");
            notes.field(&body.presence_penalty, "request.presence_penalty");
            notes.field(&body.prompt_cache_key, "request.prompt_cache_key");
            notes.field(&body.prompt_cache_options, "request.prompt_cache_options");
            notes.field(
                &body.prompt_cache_retention,
                "request.prompt_cache_retention",
            );
            notes.field(&body.reasoning_effort, "request.reasoning_effort");
            notes.field(&body.response_format, "request.response_format");
            notes.field(&body.safety_identifier, "request.safety_identifier");
            notes.field(&body.seed, "request.seed");
            notes.field(&body.service_tier, "request.service_tier");
            notes.field(&body.store, "request.store");
            notes.field(&body.stream_options, "request.stream_options");
            notes.field(&body.tool_choice, "request.tool_choice");
            notes.field(&body.top_logprobs, "request.top_logprobs");
            notes.field(&body.user, "request.user");
            notes.field(&body.verbosity, "request.verbosity");
            notes.field(&body.web_search_options, "request.web_search_options");
            notes.extra(&body.extra, "request");
        }
        Source::Responses(body) => {
            request.model = body.model.as_option().cloned();
            request.generation = Generation {
                max_output_tokens: body.max_output_tokens.as_option().copied(),
                temperature: body.temperature.as_option().copied(),
                top_p: body.top_p.as_option().copied(),
                stream: body.stream.as_option().copied().unwrap_or(false),
                ..Default::default()
            };
            if let O::Value(Input::Items(items)) = &body.input {
                for (index, item) in items.iter().enumerate() {
                    match item {
                        InputItem::FunctionCall(call) => {
                            check_status(
                                &mut notes,
                                &call.status,
                                &format!("input[{index}].status"),
                            );
                            notes.extra(&call.extra, &format!("input[{index}]"));
                        }
                        InputItem::FunctionCallOutput(output) => {
                            check_status(
                                &mut notes,
                                &output.status,
                                &format!("input[{index}].status"),
                            );
                            notes.extra(&output.extra, &format!("input[{index}]"));
                        }
                        _ => {}
                    }
                }
            }
            notes.field(&body.access_programs, "request.access_programs");
            notes.field(&body.background, "request.background");
            notes.field(&body.context_management, "request.context_management");
            notes.field(&body.conversation, "request.conversation");
            notes.field(&body.include, "request.include");
            notes.field(&body.max_tool_calls, "request.max_tool_calls");
            notes.field(&body.metadata, "request.metadata");
            notes.field(&body.moderation, "request.moderation");
            notes.field(&body.parallel_tool_calls, "request.parallel_tool_calls");
            notes.field(&body.previous_response_id, "request.previous_response_id");
            notes.field(&body.prompt, "request.prompt");
            notes.field(&body.prompt_cache_key, "request.prompt_cache_key");
            notes.field(&body.prompt_cache_options, "request.prompt_cache_options");
            notes.field(
                &body.prompt_cache_retention,
                "request.prompt_cache_retention",
            );
            notes.field(&body.reasoning, "request.reasoning");
            notes.field(&body.safety_identifier, "request.safety_identifier");
            notes.field(&body.service_tier, "request.service_tier");
            notes.field(&body.store, "request.store");
            notes.field(&body.stream_options, "request.stream_options");
            notes.field(&body.text, "request.text");
            notes.field(&body.tool_choice, "request.tool_choice");
            notes.field(&body.top_logprobs, "request.top_logprobs");
            notes.field(&body.truncation, "request.truncation");
            notes.field(&body.user, "request.user");
            notes.extra(&body.extra, "request");
        }
        Source::Messages(body) => {
            request.model = Some(body.model.clone());
            request.generation = Generation {
                max_output_tokens: Some(body.max_tokens),
                temperature: body.temperature.as_option().copied(),
                top_p: body.top_p.as_option().copied(),
                stop_sequences: body.stop_sequences.as_option().cloned(),
                stream: body.stream.as_option().copied().unwrap_or(false),
                ..Default::default()
            };
            if let O::Value(SystemPrompt::Parts(parts)) = &body.system {
                for (i, part) in parts.iter().enumerate() {
                    let path = format!("system[{i}]");
                    notes.field(&part.cache_control, &format!("{path}.cache_control"));
                    notes.field(&part.citations, &format!("{path}.citations"));
                    notes.extra(&part.extra, &path);
                }
            }
            notes.field(&body.cache_control, "request.cache_control");
            notes.field(&body.container, "request.container");
            notes.field(&body.diagnostics, "request.diagnostics");
            notes.field(&body.inference_geo, "request.inference_geo");
            notes.field(&body.metadata, "request.metadata");
            notes.field(&body.output_config, "request.output_config");
            notes.field(&body.service_tier, "request.service_tier");
            notes.field(&body.thinking, "request.thinking");
            notes.field(&body.tool_choice, "request.tool_choice");
            notes.field(&body.top_k, "request.top_k");
            notes.extra(&body.extra, "request");
        }
        Source::Gemini(body) => {
            request.model = None;
            request.generation = Generation::default();
            if let Some(config) = body.generation_config.as_option() {
                request.generation = Generation {
                    max_output_tokens: config.max_output_tokens.as_option().copied(),
                    temperature: config.temperature.as_option().copied(),
                    top_p: config.top_p.as_option().copied(),
                    stop_sequences: config.stop_sequences.as_option().cloned(),
                    candidate_count: config.candidate_count.as_option().copied(),
                    stream: false,
                };
                notes.field(
                    &config.response_mime_type,
                    "generationConfig.responseMimeType",
                );
                notes.field(&config.response_schema, "generationConfig.responseSchema");
                notes.field(
                    &config.response_json_schema,
                    "generationConfig.responseJsonSchema",
                );
                notes.field(
                    &config.response_modalities,
                    "generationConfig.responseModalities",
                );
                notes.field(&config.top_k, "generationConfig.topK");
                notes.field(&config.seed, "generationConfig.seed");
                notes.field(&config.presence_penalty, "generationConfig.presencePenalty");
                notes.field(
                    &config.frequency_penalty,
                    "generationConfig.frequencyPenalty",
                );
                notes.field(
                    &config.response_logprobs,
                    "generationConfig.responseLogprobs",
                );
                notes.field(&config.logprobs, "generationConfig.logprobs");
                notes.field(
                    &config.enable_enhanced_civic_answers,
                    "generationConfig.enableEnhancedCivicAnswers",
                );
                notes.field(&config.speech_config, "generationConfig.speechConfig");
                notes.field(&config.thinking_config, "generationConfig.thinkingConfig");
                notes.field(&config.image_config, "generationConfig.imageConfig");
                notes.field(&config.media_resolution, "generationConfig.mediaResolution");
                notes.field(
                    &config.enable_affective_dialog,
                    "generationConfig.enableAffectiveDialog",
                );
                notes.field(&config.response_format, "generationConfig.responseFormat");
                notes.field(
                    &config.translation_config,
                    "generationConfig.translationConfig",
                );
                notes.field(
                    &config.audio_transcription_config,
                    "generationConfig.audioTranscriptionConfig",
                );
                notes.extra(&config.extra, "generationConfig");
            }
            if let Some(instruction) = body.system_instruction.as_option() {
                if instruction.role.is_some() {
                    notes.dropped("systemInstruction.role");
                }
                notes.extra(&instruction.extra, "systemInstruction");
                for (i, part) in instruction.parts.iter().enumerate() {
                    let path = format!("systemInstruction.parts[{i}]");
                    notes.field(&part.thought, &format!("{path}.thought"));
                    notes.field(&part.thought_signature, &format!("{path}.thoughtSignature"));
                    notes.field(&part.part_metadata, &format!("{path}.partMetadata"));
                    notes.field(&part.media_resolution, &format!("{path}.mediaResolution"));
                    notes.field(&part.media_processing, &format!("{path}.mediaProcessing"));
                    notes.field(
                        &part.audio_transcription,
                        &format!("{path}.audioTranscription"),
                    );
                    notes.field(&part.speech_metadata, &format!("{path}.speechMetadata"));
                    notes.field(&part.inline_data, &format!("{path}.inlineData"));
                    notes.field(&part.function_call, &format!("{path}.functionCall"));
                    notes.field(&part.function_response, &format!("{path}.functionResponse"));
                    notes.field(&part.file_data, &format!("{path}.fileData"));
                    notes.field(&part.executable_code, &format!("{path}.executableCode"));
                    notes.field(
                        &part.code_execution_result,
                        &format!("{path}.codeExecutionResult"),
                    );
                    notes.field(&part.tool_call, &format!("{path}.toolCall"));
                    notes.field(&part.tool_response, &format!("{path}.toolResponse"));
                    notes.field(&part.video_metadata, &format!("{path}.videoMetadata"));
                    notes.extra(&part.extra, &path);
                }
            }

            notes.field(&body.tool_config, "request.toolConfig");
            notes.field(&body.safety_settings, "request.safetySettings");
            notes.field(&body.labels, "request.labels");
            notes.field(&body.cached_content, "request.cachedContent");
            notes.field(&body.service_tier, "request.serviceTier");
            notes.field(&body.store, "request.store");
            notes.extra(&body.extra, "request");
        }
    }
    request.tools = super::tools::decode(source, &mut notes);
    request.diagnostics = notes.0;
}
/// 未完成工具项不能作为已完成历史转换。
pub(super) fn check_status(notes: &mut Notes, status: &O<String>, path: &str) {
    if !status.is_missing() && status.as_option().is_none_or(|s| s != "completed") {
        notes.reject(path, "工具项尚未完成");
    }
}
