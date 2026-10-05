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
                max_output_tokens: body
                    .max_completion_tokens
                    .as_option()
                    .or(body.max_tokens.as_option())
                    .copied(),
                seed: body.seed.as_option().copied(),
                frequency_penalty: body.frequency_penalty.as_option().copied(),
                presence_penalty: body.presence_penalty.as_option().copied(),
                logprobs: body.logprobs.as_option().copied(),
                top_logprobs: body.top_logprobs.as_option().copied(),
                temperature: body.temperature.as_option().copied(),
                top_p: body.top_p.as_option().copied(),
                stop_sequences: body.stop.as_option().map(|stop| match stop {
                    StopSequences::One(s) => vec![s.clone()],
                    StopSequences::Many(s) => s.clone(),
                }),
                candidate_count: body.n.as_option().copied(),
                stream: body.stream.as_option().copied().unwrap_or(false),
                ..Default::default()
            };
            notes.field(&body.audio, "request.audio");
            notes.field(&body.function_call, "request.function_call");
            notes.field(&body.functions, "request.functions");
            notes.field(&body.logit_bias, "request.logit_bias");
            notes.field(&body.metadata, "request.metadata");
            notes.field(&body.modalities, "request.modalities");
            notes.field(&body.moderation, "request.moderation");
            notes.field(&body.prediction, "request.prediction");
            if let Some(options) = body.prompt_cache_options.as_option() {
                notes.extra(&options.extra, "prompt_cache_options");
            }
            notes.field(&body.safety_identifier, "request.safety_identifier");
            notes.field(&body.service_tier, "request.service_tier");
            notes.field(&body.store, "request.store");
            notes.field(&body.stream_options, "request.stream_options");

            notes.field(&body.user, "request.user");
            notes.field(&body.verbosity, "request.verbosity");

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
            // 后台生成依赖来源服务的轮询与取消接口，不能改成同步调用。
            // 参考：https://developers.openai.com/api/docs/guides/background
            if body.background.as_option() == Some(&true) {
                notes.reject(
                    "request.background",
                    "后台生成需要来源 Provider 的状态接口，不能跨协议转换",
                );
            }
            notes.field(&body.context_management, "request.context_management");
            if body.conversation.as_option().is_some() {
                notes.reject(
                    "request.conversation",
                    "此字段引用来源 Provider 的服务端状态，必须先提供完整可移植上下文",
                );
            }
            notes.field(&body.include, "request.include");
            notes.field(&body.max_tool_calls, "request.max_tool_calls");
            notes.field(&body.metadata, "request.metadata");
            notes.field(&body.moderation, "request.moderation");
            if body.previous_response_id.as_option().is_some() {
                notes.reject(
                    "request.previous_response_id",
                    "此字段引用来源 Provider 的服务端状态，必须先提供完整可移植上下文",
                );
            }
            if body.prompt.as_option().is_some() {
                notes.reject(
                    "request.prompt",
                    "此字段引用来源 Provider 的服务端状态，必须先提供完整可移植上下文",
                );
            }
            if let Some(options) = body.prompt_cache_options.as_option() {
                notes.field(
                    &options.comparison_response_id,
                    "prompt_cache_options.comparison_response_id",
                );
                notes.field(&options.prewarm, "prompt_cache_options.prewarm");
                notes.extra(&options.extra, "prompt_cache_options");
            }
            notes.field(&body.safety_identifier, "request.safety_identifier");
            notes.field(&body.service_tier, "request.service_tier");
            notes.field(&body.store, "request.store");
            notes.field(&body.stream_options, "request.stream_options");
            notes.field(&body.top_logprobs, "request.top_logprobs");
            notes.field(&body.truncation, "request.truncation");
            notes.field(&body.user, "request.user");
            notes.extra(&body.extra, "request");
        }
        Source::Messages(body) => {
            request.model = Some(body.model.clone());
            request.generation = Generation {
                max_output_tokens: Some(body.max_tokens),
                top_k: body.top_k.as_option().copied(),
                temperature: body.temperature.as_option().copied(),
                top_p: body.top_p.as_option().copied(),
                stop_sequences: body.stop_sequences.as_option().cloned(),
                stream: body.stream.as_option().copied().unwrap_or(false),
                ..Default::default()
            };
            if let O::Value(SystemPrompt::Parts(parts)) = &body.system {
                for (i, part) in parts.iter().enumerate() {
                    let path = format!("system[{i}]");

                    notes.field(&part.citations, &format!("{path}.citations"));
                    notes.extra(&part.extra, &path);
                }
            }
            if body.container.as_option().is_some() {
                notes.reject(
                    "request.container",
                    "此字段引用来源 Provider 的服务端状态，必须先提供完整可移植上下文",
                );
            }
            notes.field(&body.diagnostics, "request.diagnostics");
            notes.field(&body.inference_geo, "request.inference_geo");
            notes.field(&body.metadata, "request.metadata");
            notes.field(&body.service_tier, "request.service_tier");

            notes.extra(&body.extra, "request");
        }
        Source::Gemini(body) => {
            request.model = None;
            request.generation = Generation::default();
            if let Some(config) = body.generation_config.as_option() {
                request.generation = Generation {
                    max_output_tokens: config.max_output_tokens.as_option().copied(),
                    top_k: config.top_k.as_option().copied(),
                    seed: config.seed.as_option().copied(),
                    frequency_penalty: config.frequency_penalty.as_option().copied(),
                    presence_penalty: config.presence_penalty.as_option().copied(),
                    logprobs: config.response_logprobs.as_option().copied(),
                    top_logprobs: config.logprobs.as_option().copied(),
                    temperature: config.temperature.as_option().copied(),
                    top_p: config.top_p.as_option().copied(),
                    stop_sequences: config.stop_sequences.as_option().cloned(),
                    candidate_count: config.candidate_count.as_option().copied(),
                    stream: false,
                    ..Default::default()
                };
                notes.field(
                    &config.response_modalities,
                    "generationConfig.responseModalities",
                );
                notes.field(
                    &config.enable_enhanced_civic_answers,
                    "generationConfig.enableEnhancedCivicAnswers",
                );
                notes.field(&config.speech_config, "generationConfig.speechConfig");
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
            notes.field(&body.safety_settings, "request.safetySettings");
            notes.field(&body.labels, "request.labels");
            notes.field(&body.service_tier, "request.serviceTier");
            notes.field(&body.store, "request.store");
            notes.extra(&body.extra, "request");
        }
    }
    super::controls::decode(source, &mut request.generation, &mut notes);
    request.tools = super::tools::decode(source, &mut notes);
    request.native_tools = super::native::decode(source, &mut notes);
    super::cache::decode(request, source, &mut notes);
    // 私有输入引用承载上下文，不能按普通未知字段丢弃后继续生成。
    for (index, item) in request.items.iter().enumerate() {
        if let crate::ir::request::Item::Opaque(value) = item
            && value.get("type").and_then(serde_json::Value::as_str) == Some("item_reference")
        {
            notes.reject(&format!("items[{index}]"), "输入项引用必须先物化为完整历史");
        }
    }
    for (index, message) in request.messages.iter().enumerate() {
        for (part_index, part) in message.parts.iter().enumerate() {
            if let crate::ir::message::PartKind::Opaque(value) = &part.kind
                && value.data.get("type").and_then(serde_json::Value::as_str)
                    == Some("container_upload")
            {
                notes.reject(
                    &format!("messages[{index}].parts[{part_index}]"),
                    "容器文件引用必须先物化为可移植内容",
                );
            }
        }
    }
    request.diagnostics = notes.0;
}
/// 未完成工具项不能作为已完成历史转换。
pub(super) fn check_status(notes: &mut Notes, status: &O<String>, path: &str) {
    if !status.is_missing() && status.as_option().is_none_or(|s| s != "completed") {
        notes.reject(path, "工具项尚未完成");
    }
}
