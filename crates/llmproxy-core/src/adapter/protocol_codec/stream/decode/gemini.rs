//! Gemini 的每个分片仍是 GenerateContentResponse；相邻文本追加，函数对象按调用分块。
//! 参考：https://ai.google.dev/api/generate-content#method:-models.streamgeneratecontent

use super::Context;
use crate::{
    adapter::{Error, Result, protocol_codec::projection::finish, response::decode_gemini_usage},
    ir::{
        media::OriginalMedia,
        response::FinishReason,
        stream::{Event, Head, Key, Metadata, ToolHead},
    },
    protocol::{
        Protocol,
        gemini::{
            request::message::Part,
            response::{Response, usage::UsageMetadata},
        },
        stream::Event as Raw,
    },
};
use std::collections::BTreeMap;

/// 不把分片内重新从零编号的 parts 当成整次响应索引。
#[derive(Default)]
pub(super) struct Decoder {
    candidates: BTreeMap<u64, Candidate>,
    usage: Option<Box<UsageMetadata>>,
}

#[derive(Default)]
struct Candidate {
    next: u64,
    text: Option<(Key, bool)>,
    last_tool: Option<Key>,
    had_tool: bool,
}

impl Decoder {
    /// 只保留索引和未结束的工具；普通文本不收集完整历史。
    pub(super) fn decode(&mut self, chunk: &Response, context: &mut Context<'_>) -> Result<()> {
        if context
            .state
            .metadata()
            .and_then(|head| head.id.as_ref())
            .zip(chunk.response_id.as_option())
            .is_some_and(|(old, next)| old != next)
        {
            return Err(Error::Invalid("Gemini 流式响应 ID 改变".into()));
        }
        context.start(Metadata {
            id: chunk.response_id.as_option().cloned(),
            model: chunk.model_version.as_option().cloned(),
            created_at: None,
        })?;
        for (position, raw) in chunk
            .candidates
            .as_option()
            .into_iter()
            .flatten()
            .enumerate()
        {
            let index = raw.index.as_option().copied().unwrap_or(position as u64);
            context.candidate(index)?;
            let candidate = self.candidates.entry(index).or_default();
            if let Some(content) = raw.content.as_option() {
                for part in &content.parts {
                    candidate.part(index, part, context)?;
                }
            }
            if let Some(reason) = raw.finish_reason.as_option() {
                let mut reason = finish(Some(reason));
                if reason == FinishReason::Stop && candidate.had_tool {
                    reason = FinishReason::ToolCall;
                }
                context.close_candidate(index, reason)?;
            }
        }
        if let Some(usage) = chunk.usage_metadata.as_option() {
            let usage = self.update_usage(usage);
            context.emit(Event::Usage(decode_gemini_usage(usage).into()))?;
        }
        if chunk.candidates.as_option().is_none_or(Vec::is_empty)
            && chunk.prompt_feedback.as_option().is_some_and(|feedback| {
                feedback
                    .block_reason
                    .as_option()
                    .is_some_and(|r| r != "BLOCK_REASON_UNSPECIFIED")
            })
        {
            context.candidate(0)?;
            context.close_candidate(0, FinishReason::Filtered)?;
        }
        Ok(())
    }

    /// 先合并累计原始计数，避免后续仅报告 total 时失去 prompt 的计算上下文。
    fn update_usage(&mut self, update: &UsageMetadata) -> &UsageMetadata {
        let current = self.usage.get_or_insert_with(Default::default);
        let changed = [
            (&update.prompt_token_count, &current.prompt_token_count),
            (
                &update.candidates_token_count,
                &current.candidates_token_count,
            ),
            (&update.thoughts_token_count, &current.thoughts_token_count),
        ]
        .into_iter()
        .any(|(next, previous)| {
            next.as_option()
                .is_some_and(|next| Some(next) != previous.as_option())
        });
        if changed && update.total_token_count.as_option().is_none() {
            current.total_token_count = crate::protocol::OptionalNullable::Missing;
        }
        macro_rules! copy {
            ($($field:ident),+ $(,)?) => {$(
                if update.$field.as_option().is_some() { current.$field.clone_from(&update.$field); }
            )+};
        }
        copy!(
            prompt_token_count,
            cached_content_token_count,
            candidates_token_count,
            tool_use_prompt_token_count,
            thoughts_token_count,
            total_token_count,
            prompt_tokens_details,
            cache_tokens_details,
            candidates_tokens_details,
            tool_use_prompt_tokens_details
        );
        current
    }
}

impl Candidate {
    /// 新建全局内容索引，避免工具、思考、媒体互相覆盖。
    fn next(&mut self, candidate: u64) -> Result<Key> {
        let part = self.next;
        self.next = part
            .checked_add(1)
            .ok_or_else(|| Error::Invalid("Gemini 内容索引溢出".into()))?;
        Ok(Key {
            candidate,
            item: 0,
            part,
        })
    }

    /// 内容类型变化时关闭文本块，工具块仍等待调用组签名及候选结束。
    fn close_text(&mut self, context: &mut Context<'_>) -> Result<()> {
        if let Some((key, _)) = self.text.take() {
            context.emit(Event::PartEnd(key))?;
        }
        Ok(())
    }

    /// 签名只能附着到对应活动块，不能转成用户可见正文。
    fn signature(&self, key: Key, part: &Part, context: &mut Context<'_>) -> Result<()> {
        if let Some(data) = part.thought_signature.as_option() {
            context.emit(Event::Signature {
                key,
                protocol: Protocol::Gemini,
                data: data.clone(),
            })?;
        }
        Ok(())
    }

    /// 参数对象是局部叶子，仅将它编码为通用工具参数字符串。
    fn part(&mut self, index: u64, part: &Part, context: &mut Context<'_>) -> Result<()> {
        let fields = [
            part.text.is_missing(),
            part.function_call.is_missing(),
            part.inline_data.is_missing(),
            part.file_data.is_missing(),
            part.executable_code.is_missing(),
            part.code_execution_result.is_missing(),
            part.function_response.is_missing(),
            part.tool_call.is_missing(),
            part.tool_response.is_missing(),
        ]
        .into_iter()
        .filter(|missing| !missing)
        .count();
        if fields == 0 && part.thought_signature.as_option().is_some() {
            let key = self
                .text
                .map(|(key, _)| key)
                .or(self.last_tool)
                .ok_or_else(|| Error::Invalid("Gemini 签名没有对应内容块".into()))?;
            return self.signature(key, part, context);
        }
        if fields != 1 {
            return Err(Error::Invalid("Gemini 流式片段正文不是单一类型".into()));
        }
        if let Some(text) = part.text.as_option() {
            let thought = part.thought.as_option() == Some(&true);
            if self.text.is_some_and(|(_, previous)| previous != thought) {
                self.close_text(context)?;
            }
            let key = match self.text {
                Some((key, _)) => key,
                None => {
                    let key = self.next(index)?;
                    self.text = Some((key, thought));
                    key
                }
            };
            context.text(
                key,
                if thought { Head::Reasoning } else { Head::Text },
                text,
            )?;
            return self.signature(key, part, context);
        }
        self.close_text(context)?;
        let key = self.next(index)?;
        if let Some(call) = part.function_call.as_option() {
            if call.extra.contains_key("partialArgs") || call.extra.contains_key("willContinue") {
                return Err(Error::Unsupported(
                    "当前 Gemini API 解码器不支持 Vertex 局部参数事件".into(),
                ));
            }
            context.emit(Event::PartStart {
                key,
                head: Head::Tool(ToolHead {
                    id: call.id.as_option().cloned(),
                    name: Some(call.name.clone()),
                    text_input: false,
                }),
            })?;
            let arguments = match &call.args {
                crate::protocol::OptionalNullable::Missing => "{}".into(),
                crate::protocol::OptionalNullable::Value(args) => serde_json::to_string(args)?,
                crate::protocol::OptionalNullable::Null => {
                    return Err(Error::Invalid("Gemini 函数参数不能为 null".into()));
                }
            };
            context.emit(Event::ToolDelta {
                key,
                id: None,
                name: None,
                arguments: Some(arguments),
            })?;
            self.signature(key, part, context)?;
            self.last_tool = Some(key);
            self.had_tool = true;
        } else if part.inline_data.as_option().is_some() || part.file_data.as_option().is_some() {
            let media =
                crate::adapter::media::decode(&OriginalMedia::Gemini(Box::new(part.clone())))
                    .ok_or_else(|| Error::Unsupported("Gemini 输出媒体不能归一化".into()))?;
            context.emit(Event::Media { key, media })?;
        } else if part.executable_code.as_option().is_some()
            || part.code_execution_result.as_option().is_some()
        {
            let mut leaf = serde_json::Map::new();
            if let Some(code) = part.executable_code.as_option() {
                leaf.insert("executableCode".into(), code.clone());
            }
            if let Some(output) = part.code_execution_result.as_option() {
                leaf.insert("codeExecutionResult".into(), output.clone());
            }
            let output = crate::adapter::server_output::decode(Protocol::Gemini, &leaf)
                .ok_or_else(|| Error::Invalid("Gemini 服务端输出无法归一化".into()))?;
            context.emit(Event::ServerOutput { key, output })?;
        } else {
            // 服务端调用等没有客户端调用语义，保留原生事件让目标策略决定。
            context.emit(Event::PartStart {
                key,
                head: Head::Native(Protocol::Gemini),
            })?;
            context.emit(Event::Native(Box::new(Raw::Gemini(Box::new(Response {
                candidates: crate::protocol::OptionalNullable::Value(vec![
                    crate::protocol::gemini::response::body::Candidate {
                        index: Some(index).into(),
                        content: Some(crate::protocol::gemini::response::message::Message {
                            parts: vec![part.clone()],
                            role: Some(crate::protocol::gemini::response::message::Role::Model),
                            extra: Default::default(),
                        })
                        .into(),
                        ..Default::default()
                    },
                ]),
                ..Default::default()
            })))))?;
            context.emit(Event::PartEnd(key))?;
        }
        Ok(())
    }
}
