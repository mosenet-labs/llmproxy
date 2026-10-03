//! IR 消息直接构造 Gemini Content 和 Part。
use super::super::{Error, Result, wire};
use super::PROTOCOL;
use crate::{
    ir::request::{Message as IrMessage, PartKind, Role},
    protocol::{
        OptionalNullable as O,
        gemini::request::message::{
            FunctionCall, FunctionResponse, Message, Part, Role as RawRole,
        },
    },
};
use serde_json::Map;
use std::collections::HashMap;
/// 按调用 ID 补足工具结果名称并保持相邻用户片段顺序。
pub fn encode_gemini(messages: &[IrMessage]) -> Result<Vec<Message>> {
    let mut output: Vec<Message> = Vec::new();
    let mut call_names = HashMap::new();
    for message in messages {
        wire::reject_unmapped_source(message, PROTOCOL)?;
        let role = match message.role {
            Role::User | Role::Tool => Some(RawRole::User),
            Role::Assistant => Some(RawRole::Model),
            Role::Unspecified if wire::form(&message.metadata, PROTOCOL).is_some() => None,
            role => return Err(wire::unsupported_role(role)),
        };
        let mut parts = Vec::new();
        for part in &message.parts {
            if let PartKind::Opaque(opaque) = &part.kind {
                if opaque.protocol != PROTOCOL {
                    return Err(Error::Unsupported("无法转换不透明内容块".into()));
                }
                parts.push(serde_json::from_value(opaque.data.clone())?);
                continue;
            }
            let mut extra = wire::extra(&part.metadata, PROTOCOL);
            let call_extra = extra
                .remove("functionCall")
                .and_then(|v| v.as_object().cloned())
                .unwrap_or_default();
            let result_extra = extra
                .remove("functionResponse")
                .and_then(|v| v.as_object().cloned())
                .unwrap_or_default();
            let mut block = Part {
                thought: wire::take(&mut extra, "thought")?,
                thought_signature: wire::take(&mut extra, "thoughtSignature")?,
                part_metadata: wire::take(&mut extra, "partMetadata")?,
                media_resolution: wire::take(&mut extra, "mediaResolution")?,
                media_processing: wire::take(&mut extra, "mediaProcessing")?,
                audio_transcription: wire::take(&mut extra, "audioTranscription")?,
                speech_metadata: wire::take(&mut extra, "speechMetadata")?,
                inline_data: wire::take(&mut extra, "inlineData")?,
                file_data: wire::take(&mut extra, "fileData")?,
                executable_code: wire::take(&mut extra, "executableCode")?,
                code_execution_result: wire::take(&mut extra, "codeExecutionResult")?,
                tool_call: wire::take(&mut extra, "toolCall")?,
                tool_response: wire::take(&mut extra, "toolResponse")?,
                video_metadata: wire::take(&mut extra, "videoMetadata")?,
                extra,
                ..Default::default()
            };
            match &part.kind {
                PartKind::Text(text) => block.text = O::Value(text.clone()),
                PartKind::ToolCall(call)
                    if role == Some(RawRole::Model)
                        || (role.is_none()
                            && wire::form(&part.metadata, PROTOCOL)
                                .is_some_and(|form| form.starts_with("function_call"))) =>
                {
                    let mut extra = call_extra;
                    let original_id = wire::take(&mut extra, "id")?;
                    let id = if let Some(id) = &call.id {
                        call_names.insert(id.clone(), call.name.clone());
                        O::Value(id.clone())
                    } else {
                        original_id
                    };
                    extra.remove("args");
                    let args = if wire::form(&part.metadata, PROTOCOL)
                        == Some("function_call_no_args")
                        && call.arguments.as_object().is_some_and(Map::is_empty)
                    {
                        O::Missing
                    } else if call.arguments.is_null()
                        && wire::form(&part.metadata, PROTOCOL).is_some()
                    {
                        O::Null
                    } else {
                        O::Value(call.arguments.as_object().cloned().ok_or_else(|| {
                            Error::Unsupported("Gemini 函数参数必须是 JSON 对象".into())
                        })?)
                    };
                    block.function_call = O::Value(FunctionCall {
                        id,
                        name: call.name.clone(),
                        args,
                        extra,
                    });
                }
                PartKind::ToolResult(result)
                    if role == Some(RawRole::User)
                        || (role.is_none()
                            && wire::form(&part.metadata, PROTOCOL)
                                == Some("function_response")) =>
                {
                    let name = result
                        .name
                        .as_ref()
                        .or_else(|| result.id.as_ref().and_then(|id| call_names.get(id)))
                        .ok_or_else(|| Error::Unsupported("Gemini 函数结果缺少名称".into()))?;
                    let mut extra = result_extra;
                    let original_id = wire::take(&mut extra, "id")?;
                    let id = if let Some(id) = &result.id {
                        O::Value(id.clone())
                    } else {
                        original_id
                    };
                    block.function_response = O::Value(FunctionResponse {
                        id,
                        name: name.clone(),
                        response: result.content.as_object().cloned().unwrap_or_else(|| {
                            Map::from_iter([("result".into(), result.content.clone())])
                        }),
                        parts: wire::take(&mut extra, "parts")?,
                        will_continue: wire::take(&mut extra, "willContinue")?,
                        scheduling: wire::take(&mut extra, "scheduling")?,
                        extra,
                    });
                }
                _ => return Err(Error::Unsupported("Gemini 不支持此角色或内容块".into())),
            }
            parts.push(block);
        }
        if message.role == Role::Tool
            && let Some(Message {
                role: Some(RawRole::User),
                parts: previous,
                ..
            }) = output.last_mut()
        {
            previous.append(&mut parts);
            continue;
        }
        output.push(Message {
            role,
            parts,
            extra: wire::extra(&message.metadata, PROTOCOL),
        });
    }
    Ok(output)
}
