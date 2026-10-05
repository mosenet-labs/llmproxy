//! 四个目标协议共用生命周期、用量规范化及错误边界；各模块只构造类型化事件。
mod chat;
mod gemini;
mod messages;
mod native;
mod responses;

#[cfg(test)]
mod native_tests;
#[cfg(test)]
mod tests;

use crate::{
    adapter::{
        Error, Result,
        protocol_codec::{Conversion, ConversionWarning, ResponseTarget, cross},
    },
    ir::{
        response::FinishReason,
        stream::{Event, Head, Key, Limits, PartState, State},
        usage::Usage,
    },
    protocol::{Protocol, stream::Event as Raw},
};

/// 单次响应的目标编码器；不持有来源整包报文，失败后必须丢弃。
pub struct Encoder {
    source: Protocol,
    protocol: Protocol,
    target: Target,
    destination: Destination,
    state: State,
    limits: Limits,
    failed: bool,
}

enum Destination {
    Chat(chat::Encoder),
    Responses(responses::Encoder),
    Messages(messages::Encoder),
    Gemini(gemini::Encoder),
}

/// 外壳只在创建时复制一次，不借用短期 HTTP 上下文。
struct Target {
    model: String,
    id: String,
    created: i64,
}

impl Encoder {
    /// 创建四协议之一的编码状态，目标 ID 和模型由接入方明确提供。
    pub(crate) fn new(
        protocol: Protocol,
        source: Protocol,
        target: &ResponseTarget<'_>,
        limits: Limits,
    ) -> Result<Self> {
        if target.id.is_empty() || target.model.is_empty() {
            return Err(Error::Invalid("流式目标缺少响应 ID 或模型".into()));
        }
        if target.id.len().saturating_add(target.model.len()) > limits.buffered_bytes {
            return Err(Error::Invalid("流式目标外壳超过缓冲上限".into()));
        }
        let destination = match protocol {
            Protocol::OpenAiChat => Destination::Chat(Default::default()),
            Protocol::OpenAiResponses => Destination::Responses(Default::default()),
            Protocol::AnthropicMessages => Destination::Messages(Default::default()),
            Protocol::Gemini => Destination::Gemini(Default::default()),
        };
        Ok(Self {
            source,
            protocol,
            target: Target {
                model: target.model.into(),
                id: target.id.into(),
                created: target.created,
            },
            destination,
            state: State::new(limits),
            limits,
            failed: false,
        })
    }

    /// 校验单个 IR 事件并立即生成目标增量；仅工具参数及 Responses 快照需要暂存。
    pub fn push(&mut self, event: &Event) -> Result<Conversion<Vec<Raw>>> {
        if self.failed {
            return Err(Error::Invalid("流式编码器失败后不能复用".into()));
        }
        let result = self.encode(event);
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    /// 返回上游真实用量及语义结束状态，不含 Messages 的协议占位值。
    pub fn state(&self) -> &State {
        &self.state
    }

    /// 统计参数及终态快照保留的正文长度；结构体和索引的开销另受 entries 限制。
    pub fn buffered_bytes(&self) -> usize {
        self.state.buffered_bytes()
            + match &self.destination {
                Destination::Responses(destination) => destination.buffered_bytes(),
                _ => 0,
            }
    }

    /// 先校验再交付结果；PartEnd 的工具参数在状态释放前保存一次局部快照。
    fn encode(&mut self, event: &Event) -> Result<Conversion<Vec<Raw>>> {
        if let (Some(status), Event::End(next)) = (self.state.ended(), event)
            && status == *next
        {
            return Ok(Conversion::exact(Vec::new()));
        }
        let closing = match event {
            Event::PartEnd(key) => self.state.part(*key).cloned(),
            _ => None,
        };
        self.state
            .accept(event)
            .map_err(|error| Error::Invalid(error.to_string()))?;
        let mut context = Context {
            source: self.source,
            protocol: self.protocol,
            target: &self.target,
            state: &self.state,
            closing: closing.as_ref(),
            events: Vec::new(),
            warnings: Vec::new(),
            remaining: self
                .limits
                .buffered_bytes
                .saturating_sub(self.state.buffered_bytes()),
        };
        // 单候选协议在增量发送前固定选择 0；不能等待整条流再寻找最小索引。
        if matches!(
            self.protocol,
            Protocol::OpenAiResponses | Protocol::AnthropicMessages
        ) && candidate(event).is_some_and(|index| index != 0)
        {
            context.warn(
                "candidates",
                "目标协议只支持单候选，流式保留候选 0；用量仍为全部候选合计",
            );
            return Ok(context.finish());
        }
        if matches!(event, Event::End(_))
            && matches!(
                self.protocol,
                Protocol::OpenAiResponses | Protocol::AnthropicMessages
            )
            && self.state.candidate(0).is_none()
            && !matches!(
                self.state.ended(),
                Some(crate::ir::response::Status::Failed | crate::ir::response::Status::Cancelled)
            )
        {
            return Err(unsupported("candidates", "单候选目标没有候选 0"));
        }
        match event {
            Event::Diagnostic {
                protocol,
                diagnostic,
            } => {
                if *protocol != self.source {
                    return Err(Error::Invalid("流式诊断与来源协议不匹配".into()));
                }
                if diagnostic.reject {
                    return Err(unsupported(&diagnostic.path, &diagnostic.reason));
                }
                context.warn(&diagnostic.path, &diagnostic.reason);
                return Ok(context.finish());
            }
            Event::PartStart {
                head: Head::Native(protocol),
                ..
            } => {
                if *protocol != self.source {
                    return Err(Error::Invalid("原生内容块与来源协议不匹配".into()));
                }
                return Ok(context.finish());
            }
            Event::PartEnd(key) if matches!(context.part(*key)?.head, Head::Native(_)) => {
                return Ok(context.finish());
            }
            Event::Native(raw) => {
                native::encode(&mut self.destination, raw, &mut context)?;
                return Ok(context.finish());
            }
            Event::ServerOutput { key, output } => {
                if let Some(text) = cross::server_output::text(
                    output,
                    self.source,
                    self.protocol,
                    "server_output",
                    &mut context.warnings,
                ) {
                    // IR 中的服务端输出是已结束的原子块，目标拆成三条事件，不重复写入 IR 状态。
                    for projected in [
                        Event::PartStart {
                            key: *key,
                            head: Head::Text,
                        },
                        Event::TextDelta { key: *key, text },
                        Event::PartEnd(*key),
                    ] {
                        dispatch(&mut self.destination, &projected, &mut context)?;
                    }
                }
                return Ok(context.finish());
            }
            Event::Unknown { protocol, .. } => {
                if *protocol == self.protocol {
                    return Err(unsupported(
                        "stream.unknown",
                        "未知事件的顺序号与内容索引无法安全重建；同协议 HTTP 直通可保留",
                    ));
                } else {
                    context.warn("stream.unknown", "未知事件无目标语义，已丢弃");
                }
                return Ok(context.finish());
            }
            _ => {}
        }
        dispatch(&mut self.destination, event, &mut context)?;
        Ok(context.finish())
    }
}

/// 通用增量与原子服务端输出共用同一个目标事件构造入口。
fn dispatch(destination: &mut Destination, event: &Event, context: &mut Context<'_>) -> Result<()> {
    match destination {
        Destination::Chat(destination) => destination.encode(event, context),
        Destination::Responses(destination) => destination.encode(event, context),
        Destination::Messages(destination) => destination.encode(event, context),
        Destination::Gemini(destination) => gemini::encode(destination, event, context),
    }
}

/// 提取事件所属候选；统计、失败和生命周期不属于某一个候选。
fn candidate(event: &Event) -> Option<u64> {
    match event {
        Event::CandidateStart(index) | Event::CandidateEnd { index, .. } => Some(*index),
        Event::PartStart { key, .. }
        | Event::TextDelta { key, .. }
        | Event::ToolDelta { key, .. }
        | Event::Signature { key, .. }
        | Event::PartEnd(key)
        | Event::Media { key, .. }
        | Event::ServerOutput { key, .. }
        | Event::Annotation { key, .. } => Some(key.candidate),
        _ => None,
    }
}

/// 所有目标模块共用的外壳、真实统计和安全诊断入口。
struct Context<'a> {
    source: Protocol,
    protocol: Protocol,
    target: &'a Target,
    state: &'a State,
    closing: Option<&'a PartState>,
    events: Vec<Raw>,
    warnings: Vec<ConversionWarning>,
    remaining: usize,
}

impl Context<'_> {
    /// 只输出静态字段路径与说明，不记录文本、参数或签名。
    fn warn(&mut self, path: &str, reason: &str) {
        self.warnings.push(ConversionWarning {
            source: self.source,
            target: self.protocol,
            path: path.into(),
            reason: reason.into(),
        });
    }
    /// 借用工具关闭前的局部状态；其他事件读取当前活动块。
    fn part(&self, key: Key) -> Result<&PartState> {
        self.closing
            .or_else(|| self.state.part(key))
            .ok_or_else(|| Error::Invalid("目标内容块不存在".into()))
    }
    /// 共用非流式的用量校验和细分降级规则，累计事件从完整快照映射。
    fn usage(&mut self) -> Result<Option<Usage>> {
        self.state
            .usage()
            .snapshot()
            .map(|usage| {
                cross::usage::normalize(usage, self.source, self.protocol, &mut self.warnings)
            })
            .transpose()
    }
    /// 无调用 ID 的来源使用稳定且有响应作用域的目标标识，不使用工具参数生成标识。
    fn call_id(&self, key: Key, head: &crate::ir::stream::ToolHead) -> String {
        head.id
            .clone()
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| self.item_id(key))
    }
    /// 重建目标输出项标识，不复用另一个协议的索引外壳。
    fn item_id(&self, key: Key) -> String {
        format!(
            "{}_{}_{}_{}",
            self.target.id, key.candidate, key.item, key.part
        )
    }
    /// 仅在完整参数已生成时解析开放的 JSON 参数叶子。
    fn arguments(&self, part: &PartState) -> Result<serde_json::Map<String, serde_json::Value>> {
        let Head::Tool(head) = &part.head else {
            return Err(Error::Invalid("目标内容块不是工具".into()));
        };
        if head.text_input {
            return Err(unsupported(
                "tool.input",
                "目标函数工具不能表达自由文本输入",
            ));
        }
        serde_json::from_str(&part.arguments).map_err(|_| {
            unsupported(
                "tool.arguments",
                "目标需要完整 JSON 参数对象，不能发送截断或非对象参数",
            )
        })
    }
    /// 交还类型化事件和本次降级诊断。
    fn finish(self) -> Conversion<Vec<Raw>> {
        Conversion {
            body: self.events,
            warnings: self.warnings,
        }
    }
}

/// 目标协议不能安全表达的内容以静态路径报错。
fn unsupported(path: &str, reason: &str) -> Error {
    Error::Unsupported(format!("{path}: {reason}"))
}

/// 三种完成协议共用停止原因分类，Responses 在终态快照中表达 Length/Filtered。
fn finish(protocol: Protocol, reason: FinishReason) -> Result<&'static str> {
    use FinishReason::*;
    match (protocol, reason) {
        (_, Unknown) => Err(unsupported(
            "finish_reason",
            "未知结束原因不能编码为正常完成",
        )),
        (Protocol::OpenAiChat, Stop | Refusal) => Ok("stop"),
        (Protocol::OpenAiChat, ToolCall) => Ok("tool_calls"),
        (Protocol::OpenAiChat, Length) => Ok("length"),
        (Protocol::OpenAiChat, Filtered) => Ok("content_filter"),
        (Protocol::AnthropicMessages, Stop) => Ok("end_turn"),
        (Protocol::AnthropicMessages, ToolCall) => Ok("tool_use"),
        (Protocol::AnthropicMessages, Length) => Ok("max_tokens"),
        (Protocol::AnthropicMessages, Filtered | Refusal) => Ok("refusal"),
        (Protocol::Gemini, Stop | ToolCall | Refusal) => Ok("STOP"),
        (Protocol::Gemini, Length) => Ok("MAX_TOKENS"),
        (Protocol::Gemini, Filtered) => Ok("SAFETY"),
        (Protocol::OpenAiResponses, _) => {
            Err(Error::Invalid("Responses 使用响应状态表达终止原因".into()))
        }
    }
}
