//! Chat 候选的文本及工具参数都是追加片段；用量通常在空 choices 分片中报告。
//! 参考：https://developers.openai.com/api/reference/resources/chat/subresources/completions/streaming-events

use super::Context;
use crate::{
    adapter::protocol_codec::projection::finish,
    adapter::{Error, Result, response::decode_chat_usage},
    ir::stream::{Event, Head, Key, Metadata, ToolHead},
    protocol::{chat::response::Chunk, stream::Event as Raw},
};

/// 一个 Chunk 可以同时更新多个候选和多个并行工具。
pub(super) fn decode(chunk: &Chunk, context: &mut Context<'_>) -> Result<()> {
    if chunk.object != "chat.completion.chunk" {
        return Err(Error::Invalid("不是 Chat 流式分片".into()));
    }
    if context
        .state
        .metadata()
        .is_some_and(|head| head.id.as_deref() != Some(chunk.id.as_str()))
    {
        return Err(Error::Invalid("Chat 流式响应 ID 改变".into()));
    }
    context.start(Metadata {
        id: Some(chunk.id.clone()),
        model: Some(chunk.model.clone()),
        created_at: Some(chunk.created),
    })?;
    for choice in &chunk.choices {
        let index = choice.index;
        context.candidate(index)?;
        let delta = &choice.delta;
        if delta
            .role
            .as_option()
            .is_some_and(|role| role != "assistant")
        {
            return Err(Error::Invalid("Chat 回复增量角色不是 assistant".into()));
        }
        if let Some(text) = delta.content.as_option() {
            context.text(
                Key {
                    candidate: index,
                    item: 0,
                    part: 0,
                },
                Head::Text,
                text,
            )?;
        }
        if let Some(text) = delta.refusal.as_option() {
            context.text(
                Key {
                    candidate: index,
                    item: 0,
                    part: 1,
                },
                Head::Refusal,
                text,
            )?;
        }
        if let Some(calls) = delta.tool_calls.as_option() {
            if delta.function_call.as_option().is_some() {
                return Err(Error::Invalid("Chat 增量同时包含两种函数调用格式".into()));
            }
            for call in calls {
                if call
                    .r#type
                    .as_option()
                    .is_some_and(|kind| kind != "function")
                {
                    return Err(Error::Unsupported("Chat 流式工具类型尚无事件映射".into()));
                }
                let item = call
                    .index
                    .checked_add(1)
                    .ok_or_else(|| Error::Invalid("工具索引溢出".into()))?;
                let key = Key {
                    candidate: index,
                    item,
                    part: 0,
                };
                context.part(key, Head::Tool(ToolHead::default()))?;
                context.emit(Event::ToolDelta {
                    key,
                    id: call.id.as_option().cloned(),
                    name: call
                        .function
                        .as_option()
                        .and_then(|f| f.name.as_option())
                        .cloned(),
                    arguments: call
                        .function
                        .as_option()
                        .and_then(|f| f.arguments.as_option())
                        .cloned(),
                })?;
            }
        }
        if let Some(call) = delta.function_call.as_option() {
            let key = Key {
                candidate: index,
                item: 1,
                part: 0,
            };
            context.part(key, Head::Tool(ToolHead::default()))?;
            context.emit(Event::ToolDelta {
                key,
                id: None,
                name: call.name.as_option().cloned(),
                arguments: call.arguments.as_option().cloned(),
            })?;
        }
        if let Some(reason) = choice.finish_reason.as_option() {
            context.close_candidate(index, finish(Some(reason)))?;
        }
    }
    if let Some(usage) = chunk.usage.as_option() {
        context.emit(Event::Usage(decode_chat_usage(usage).into()))?;
    }
    // 音频没有输出编码上下文，不能猜测格式；保留类型化原生载体供目标策略处理。
    if chunk
        .choices
        .iter()
        .any(|c| c.delta.audio.as_option().is_some())
    {
        context.emit(Event::Native(Box::new(Raw::Chat(Box::new(chunk.clone())))))?;
    }
    Ok(())
}
