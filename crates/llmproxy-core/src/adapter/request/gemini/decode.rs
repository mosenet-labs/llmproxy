//! Gemini 请求内容的直接类型投影。
use super::super::{Result, wire};
use super::PROTOCOL;
use crate::{
    ir::request::{Message as IrMessage, PartKind, Role, ToolCall, ToolResult},
    protocol::{
        OptionalNullable as O,
        gemini::request::message::{Message, Role as RawRole},
    },
};
use serde_json::{Map, Value};
/// 对互斥正文读取类型字段，缺失角色和不透明块保持原样。
pub fn decode_gemini(messages: &[Message]) -> Result<Vec<IrMessage>> {
    messages
        .iter()
        .map(|m| {
            let role = match m.role {
                Some(RawRole::User) => Role::User,
                Some(RawRole::Model) => Role::Assistant,
                None => Role::Unspecified,
            };
            let mut parts = Vec::new();
            for block in &m.parts {
                let count = [
                    !block.text.is_missing(),
                    !block.function_call.is_missing(),
                    !block.function_response.is_missing(),
                    !block.inline_data.is_missing(),
                    !block.file_data.is_missing(),
                    !block.tool_call.is_missing(),
                    !block.tool_response.is_missing(),
                    !block.executable_code.is_missing(),
                    !block.code_execution_result.is_missing(),
                ]
                .into_iter()
                .filter(|present| *present)
                .count();
                if count != 1 {
                    parts.push(wire::opaque_value(PROTOCOL, block)?);
                    continue;
                }
                let mut extra = block.extra.clone();
                wire::put(&mut extra, "thought", &block.thought)?;
                wire::put(&mut extra, "thoughtSignature", &block.thought_signature)?;
                wire::put(&mut extra, "partMetadata", &block.part_metadata)?;
                wire::put(&mut extra, "mediaResolution", &block.media_resolution)?;
                wire::put(&mut extra, "mediaProcessing", &block.media_processing)?;
                wire::put(&mut extra, "audioTranscription", &block.audio_transcription)?;
                wire::put(&mut extra, "speechMetadata", &block.speech_metadata)?;
                wire::put(&mut extra, "inlineData", &block.inline_data)?;
                wire::put(&mut extra, "fileData", &block.file_data)?;
                wire::put(&mut extra, "executableCode", &block.executable_code)?;
                wire::put(
                    &mut extra,
                    "codeExecutionResult",
                    &block.code_execution_result,
                )?;
                wire::put(&mut extra, "toolCall", &block.tool_call)?;
                wire::put(&mut extra, "toolResponse", &block.tool_response)?;
                wire::put(&mut extra, "videoMetadata", &block.video_metadata)?;

                let part = if let Some(text) = block.text.as_option() {
                    if block.thought == O::Value(true) {
                        wire::part(
                            PartKind::Reasoning(Value::String(text.clone())),
                            PROTOCOL,
                            "thinking",
                            extra,
                        )
                    } else {
                        wire::text_part(text.clone(), PROTOCOL, "text", extra)
                    }
                } else if let Some(call) = block.function_call.as_option() {
                    let mut nested = call.extra.clone();
                    if matches!(call.id, O::Null) {
                        nested.insert("id".into(), Value::Null);
                    }
                    if matches!(call.args, O::Null) {
                        nested.insert("args".into(), Value::Null);
                    }
                    if !nested.is_empty() {
                        extra.insert("functionCall".into(), Value::Object(nested));
                    }
                    let args = match &call.args {
                        O::Value(args) => Value::Object(args.clone()),
                        O::Null => Value::Null,
                        O::Missing => Value::Object(Map::new()),
                    };
                    wire::part(
                        PartKind::ToolCall(ToolCall {
                            id: call.id.as_option().cloned(),
                            name: call.name.clone(),
                            arguments: args,
                        }),
                        PROTOCOL,
                        if call.args.is_missing() {
                            "function_call_no_args"
                        } else {
                            "function_call"
                        },
                        extra,
                    )
                } else if let Some(result) = block.function_response.as_option() {
                    let mut nested = result.extra.clone();
                    if matches!(result.id, O::Null) {
                        nested.insert("id".into(), Value::Null);
                    }
                    wire::put(&mut nested, "parts", &result.parts)?;
                    wire::put(&mut nested, "willContinue", &result.will_continue)?;
                    wire::put(&mut nested, "scheduling", &result.scheduling)?;
                    if !nested.is_empty() {
                        extra.insert("functionResponse".into(), Value::Object(nested));
                    }
                    wire::part(
                        PartKind::ToolResult(ToolResult {
                            id: result.id.as_option().cloned(),
                            name: Some(result.name.clone()),
                            content: Value::Object(result.response.clone()),
                        }),
                        PROTOCOL,
                        "function_response",
                        extra,
                    )
                } else {
                    crate::adapter::media::part(crate::ir::media::OriginalMedia::Gemini(Box::new(
                        block.clone(),
                    )))?
                };
                parts.push(part);
            }
            Ok(wire::message(
                role,
                parts,
                PROTOCOL,
                "parts",
                m.extra.clone(),
            ))
        })
        .collect()
}
