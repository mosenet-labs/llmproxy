//! 原生事件按已知语义分类；不回放包含已投影正文的整条来源载体。
use super::{Context, Destination, unsupported};
use crate::{
    adapter::{Error, Result},
    protocol::{
        responses::response::{Event, event::KnownEvent as E},
        stream::Event as Raw,
    },
};

/// 服务端进度降级为诊断，完成输出另从快照进入 ServerOutput；媒体片段不静默丢弃。
pub(super) fn encode(
    destination: &mut Destination,
    raw: &Raw,
    context: &mut Context<'_>,
) -> Result<()> {
    if raw.protocol() != context.source {
        return Err(Error::Invalid("原生事件与来源协议不匹配".into()));
    }
    if let Raw::Chat(chunk) = raw {
        if let Destination::Chat(destination) = destination {
            for choice in &chunk.choices {
                if context.state.candidate(choice.index).is_none() {
                    return Err(Error::Invalid("原生 Chat 音频缺少候选".into()));
                }
                if choice.delta.content.as_option().is_some()
                    || choice.delta.refusal.as_option().is_some()
                    || choice.delta.tool_calls.as_option().is_some()
                    || choice.delta.function_call.as_option().is_some()
                    || choice.delta.role.as_option().is_some()
                    || choice.finish_reason.as_option().is_some()
                    || chunk.usage.as_option().is_some()
                {
                    return Err(unsupported(
                        "stream.native",
                        "原生 Chat 音频载体不能包含已投影的通用字段",
                    ));
                }
                let audio = choice
                    .delta
                    .audio
                    .as_option()
                    .ok_or_else(|| unsupported("audio", "原生 Chat 分片缺少音频"))?;
                destination.audio(choice.index, audio, context);
            }
            return Ok(());
        }
        return Err(unsupported(
            "audio",
            "音频增量缺少可移植编码格式，不能跨协议发送",
        ));
    }
    if let Raw::Responses(event) = raw
        && let Event::Known(event) = event.as_ref()
    {
        match event.as_ref() {
            E::FileSearchInProgress(_)
            | E::FileSearchSearching(_)
            | E::FileSearchCompleted(_)
            | E::WebSearchInProgress(_)
            | E::WebSearchSearching(_)
            | E::WebSearchCompleted(_)
            | E::CodeInterpreterInProgress(_)
            | E::CodeInterpreterInterpreting(_)
            | E::CodeInterpreterCompleted(_)
            | E::CodeInterpreterCodeDelta(_)
            | E::CodeInterpreterCodeDone(_)
            | E::McpCallInProgress(_)
            | E::McpCallCompleted(_)
            | E::McpCallFailed(_)
            | E::McpListToolsInProgress(_)
            | E::McpListToolsCompleted(_)
            | E::McpListToolsFailed(_)
            | E::McpArgumentsDelta(_)
            | E::McpArgumentsDone(_)
            | E::ShellCommandAdded(_)
            | E::ShellCommandDelta(_)
            | E::ShellCommandDone(_)
            | E::ShellOutputDelta(_)
            | E::ShellOutputDone(_)
            | E::CompactionCompacting(_)
            | E::ImageGenerationInProgress(_)
            | E::ImageGenerationGenerating(_)
            | E::ImageGenerationCompleted(_) => {
                context.warn(
                    &format!("events.{}", event.kind()),
                    "来源服务端执行进度未复制；可见完成输出由独立快照映射",
                );
                return Ok(());
            }
            E::ImageGenerationPartial(_) => {
                return Err(unsupported(
                    "image_generation.partial_image",
                    "图片预览缺少完整媒体载体，不能当作文本或完整图片发送",
                ));
            }
            E::AudioDelta(_)
            | E::AudioDone(_)
            | E::AudioTranscriptDelta(_)
            | E::AudioTranscriptDone(_) => {
                if !matches!(context.state.candidate(0), Some(None)) {
                    return Err(Error::Invalid("Responses 原生音频没有活动候选".into()));
                }
                if let Destination::Responses(destination) = destination {
                    return destination.audio(event, context);
                }
                return Err(unsupported(
                    "audio",
                    "原生音频事件缺少可移植编码格式，不能跨协议发送",
                ));
            }
            _ => {}
        }
    }
    Err(unsupported(
        "stream.native",
        "原生内容没有可安全重建的目标载体",
    ))
}
