//! 不依赖来源报文构造目标事件；仅专属或未知事件保留类型化来源。
use serde::{Deserialize, Serialize};

use crate::{
    ir::{
        media::Media,
        response::{Failure, FinishReason, Status},
        server_output::ServerOutput,
        usage::Usage,
    },
    protocol::{
        Protocol,
        stream::{Event as ProtocolEvent, UnknownEvent},
    },
};

/// 内容的位置，避免多候选、多个输出项和并行工具使用同一个索引。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Key {
    /// 候选索引；单候选协议使用 0。
    pub candidate: u64,
    /// 候选中的输出项索引。
    pub item: u64,
    /// 输出项中的内容块索引。
    pub part: u64,
}

/// 响应的通用外壳，缺失值不由 codec 猜测。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Metadata {
    /// 来源响应 ID；接入边界可为目标协议提供独立 ID。
    pub id: Option<String>,
    /// 实际响应模型。
    pub model: Option<String>,
    /// Unix 秒级创建时间。
    pub created_at: Option<i64>,
}

/// 工具的初始信息，后续名字或 ID 可以在增量中出现。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ToolHead {
    /// 工具调用 ID，未报告时保持缺失。
    pub id: Option<String>,
    /// 初始工具名；不能以空名字发送目标协议的完整工具头。
    pub name: Option<String>,
    /// 是否是自定义工具的普通文本输入；函数参数默认为 JSON 字符串。
    pub text_input: bool,
}

/// 可按字符串增量生成的内容类型。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Head {
    /// 可见文本。
    Text,
    /// 拒绝说明。
    Refusal,
    /// 思考正文或摘要。
    Reasoning,
    /// 等待客户端执行的工具调用，区别于服务端工具输出。
    Tool(ToolHead),
    /// 具有块边界、但尚未归一化的原生内容；跨协议编码需要显式处理。
    Native(Protocol),
}

/// 一条具有明确顺序语义的 IR 事件。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Event {
    /// 整个响应开始。
    Start(Metadata),
    /// 一个候选开始。
    CandidateStart(u64),
    /// 一个可增量输出的内容块开始。
    PartStart { key: Key, head: Head },
    /// 文本、拒绝或思考正文追加。
    TextDelta { key: Key, text: String },
    /// 工具 ID、名称和参数的局部更新，不要求参数是完整 JSON。
    ToolDelta {
        key: Key,
        id: Option<String>,
        name: Option<String>,
        arguments: Option<String>,
    },
    /// 不透明签名只可用于匹配来源协议，不能作为可见文本或跨协议签名。
    Signature {
        key: Key,
        protocol: Protocol,
        data: String,
    },
    /// 一个内容块完成，不等同于整个候选完成。
    PartEnd(Key),
    /// 完整媒体载体；无等价目标载体时不得静默丢弃字节。
    Media { key: Key, media: Media },
    /// 服务端已经执行的可见输出，不触发客户端工具执行。
    ServerOutput { key: Key, output: ServerOutput },
    /// 内容引用等开放叶子，仍需要目标协议决定其表达方式。
    Annotation {
        key: Key,
        annotation: serde_json::Value,
    },
    /// 候选语义结束；传输可能还会发送用量。
    CandidateEnd { index: u64, reason: FinishReason },
    /// 局部累计用量，字段有值时覆盖，不重复求和。
    Usage(Box<Usage>),
    /// 心跳不产生正文，也不表示生成完成。
    Heartbeat,
    /// 生成或传输失败，错误说明不应直接进入日志。
    Failure(Failure),
    /// 正常或失败的整条流结束。
    End(Status),
    /// 仅用于尚未规范化的原生事件；跨协议编码必须显式处理或拒绝。
    Native(Box<ProtocolEvent>),
    /// 真正未知的事件，用于同协议保留或跨协议诊断。
    Unknown {
        protocol: Protocol,
        event: UnknownEvent,
    },
}
