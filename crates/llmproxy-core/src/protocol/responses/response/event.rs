//! Responses 的类型化 SSE 事件；增量与完整快照不混用。
//! 参考 API：https://developers.openai.com/api/reference/resources/responses/streaming-events
use super::body::{OutputItem, Response};
use crate::protocol::{
    OptionalNullable as O,
    stream::{self, UnknownEvent},
};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

/// 已知事件严格读取自己的载体，未知事件保留开放扩展。
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Event {
    /// 已声明的响应事件。
    Known(Box<KnownEvent>),
    /// Provider 新增的事件。
    Other(UnknownEvent),
}
impl<'de> Deserialize<'de> for Event {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match stream::decode_open(deserializer, KnownEvent::is_known)? {
            stream::Open::Known(event) => Ok(Self::Known(event)),
            stream::Open::Other(event) => Ok(Self::Other(event)),
        }
    }
}
stream::typed_events! {
    /// Responses 事件名与其独立的载体。
    pub enum KnownEvent {
        /// 响应创建。
        Created(Lifecycle) = "response.created",
        /// 响应开始生成。
        InProgress(Lifecycle) = "response.in_progress",
        /// 响应生成完成。
        Completed(Lifecycle) = "response.completed",
        /// 响应生成失败。
        Failed(Lifecycle) = "response.failed",
        /// 响应未完整生成。
        Incomplete(Lifecycle) = "response.incomplete",
        /// 新增输出项。
        OutputItemAdded(ItemEvent) = "response.output_item.added",
        /// 输出项完成。
        OutputItemDone(ItemEvent) = "response.output_item.done",
        /// 新增输出内容块。
        ContentPartAdded(ContentEvent) = "response.content_part.added",
        /// 输出内容块完成。
        ContentPartDone(ContentEvent) = "response.content_part.done",
        /// 可见文本增量。
        OutputTextDelta(ContentDelta) = "response.output_text.delta",
        /// 完整可见文本。
        OutputTextDone(TextDone) = "response.output_text.done",
        /// 拒绝文本增量。
        RefusalDelta(ContentDelta) = "response.refusal.delta",
        /// 完整拒绝文本。
        RefusalDone(RefusalDone) = "response.refusal.done",
        /// 函数参数字符串增量。
        FunctionArgumentsDelta(ItemDelta) = "response.function_call_arguments.delta",
        /// 完整函数参数字符串。
        FunctionArgumentsDone(ArgumentsDone) = "response.function_call_arguments.done",
        /// 服务端工具状态：file_search_call / in_progress。
        FileSearchInProgress(ItemStatus) = "response.file_search_call.in_progress",
        /// 服务端工具状态：file_search_call / searching。
        FileSearchSearching(ItemStatus) = "response.file_search_call.searching",
        /// 服务端工具状态：file_search_call / completed。
        FileSearchCompleted(ItemStatus) = "response.file_search_call.completed",
        /// 服务端工具状态：web_search_call / in_progress。
        WebSearchInProgress(ItemStatus) = "response.web_search_call.in_progress",
        /// 服务端工具状态：web_search_call / searching。
        WebSearchSearching(ItemStatus) = "response.web_search_call.searching",
        /// 服务端工具状态：web_search_call / completed。
        WebSearchCompleted(ItemStatus) = "response.web_search_call.completed",
        /// 服务端工具状态：image_generation_call / completed。
        ImageGenerationCompleted(ItemStatus) = "response.image_generation_call.completed",
        /// 服务端工具状态：image_generation_call / generating。
        ImageGenerationGenerating(ItemStatus) = "response.image_generation_call.generating",
        /// 服务端工具状态：image_generation_call / in_progress。
        ImageGenerationInProgress(ItemStatus) = "response.image_generation_call.in_progress",
        /// 服务端工具状态：mcp_call / completed。
        McpCallCompleted(ItemStatus) = "response.mcp_call.completed",
        /// 服务端工具状态：mcp_call / failed。
        McpCallFailed(ItemStatus) = "response.mcp_call.failed",
        /// 服务端工具状态：mcp_call / in_progress。
        McpCallInProgress(ItemStatus) = "response.mcp_call.in_progress",
        /// 服务端工具状态：mcp_list_tools / completed。
        McpListToolsCompleted(ItemStatus) = "response.mcp_list_tools.completed",
        /// 服务端工具状态：mcp_list_tools / failed。
        McpListToolsFailed(ItemStatus) = "response.mcp_list_tools.failed",
        /// 服务端工具状态：mcp_list_tools / in_progress。
        McpListToolsInProgress(ItemStatus) = "response.mcp_list_tools.in_progress",
        /// 服务端工具状态：code_interpreter_call / in_progress。
        CodeInterpreterInProgress(ItemStatus) = "response.code_interpreter_call.in_progress",
        /// 服务端工具状态：code_interpreter_call / interpreting。
        CodeInterpreterInterpreting(ItemStatus) = "response.code_interpreter_call.interpreting",
        /// 服务端工具状态：code_interpreter_call / completed。
        CodeInterpreterCompleted(ItemStatus) = "response.code_interpreter_call.completed",
        /// 新增思考摘要块。
        ReasoningSummaryPartAdded(SummaryPart) = "response.reasoning_summary_part.added",
        /// 思考摘要块完成。
        ReasoningSummaryPartDone(SummaryPart) = "response.reasoning_summary_part.done",
        /// 思考摘要文本增量。
        ReasoningSummaryTextDelta(SummaryDelta) = "response.reasoning_summary_text.delta",
        /// 完整思考摘要文本。
        ReasoningSummaryTextDone(SummaryDone) = "response.reasoning_summary_text.done",
        /// 思考正文增量。
        ReasoningTextDelta(ContentDelta) = "response.reasoning_text.delta",
        /// 完整思考正文。
        ReasoningTextDone(TextDone) = "response.reasoning_text.done",
        /// 部分图片的完整 Base64 载体。
        ImageGenerationPartial(PartialImage) = "response.image_generation_call.partial_image",
        /// MCP 参数增量。
        McpArgumentsDelta(ItemDelta) = "response.mcp_call_arguments.delta",
        /// 完整 MCP 参数。
        McpArgumentsDone(ArgumentsDone) = "response.mcp_call_arguments.done",
        /// 服务端执行代码增量。
        CodeInterpreterCodeDelta(ItemDelta) = "response.code_interpreter_call_code.delta",
        /// 完整服务端执行代码。
        CodeInterpreterCodeDone(CodeDone) = "response.code_interpreter_call_code.done",
        /// 文本引用标注。
        AnnotationAdded(AnnotationAdded) = "response.output_text.annotation.added",
        /// 响应排队。
        Queued(Lifecycle) = "response.queued",
        /// 自定义工具输入增量。
        CustomToolInputDelta(ItemDelta) = "response.custom_tool_call_input.delta",
        /// 完整自定义工具输入。
        CustomToolInputDone(InputDone) = "response.custom_tool_call_input.done",
        /// 流内失败。
        Error(ErrorEvent) = "error",
        /// 音频字节的 Base64 分片。
        AudioDelta(AudioDelta) = "response.audio.delta",
        /// 音频生成结束。
        AudioDone(AudioDone) = "response.audio.done",
        /// 音频转录文本增量。
        AudioTranscriptDelta(AudioDelta) = "response.audio.transcript.delta",
        /// 音频转录结束。
        AudioTranscriptDone(AudioDone) = "response.audio.transcript.done",
        /// 上下文压缩进度，未携带摘要正文。
        CompactionCompacting(ItemStatus) = "response.compaction.compacting",
        /// 新增服务端 shell 命令。
        ShellCommandAdded(ShellCommand) = "response.shell_call_command.added",
        /// 服务端 shell 命令增量。
        ShellCommandDelta(ShellCommandDelta) = "response.shell_call_command.delta",
        /// 完整服务端 shell 命令。
        ShellCommandDone(ShellCommand) = "response.shell_call_command.done",
        /// 服务端 stdout/stderr 增量。
        ShellOutputDelta(ShellOutputDelta) = "response.shell_call_output_content.delta",
        /// 完整服务端执行输出。
        ShellOutputDone(ShellOutputDone) = "response.shell_call_output_content.done",
    }
}

impl KnownEvent {
    /// 读取所有已知事件的顺序号，避免通过 JSON 投影取得通用字段。
    pub fn sequence_number(&self) -> u64 {
        use KnownEvent::*;
        match self {
            Created(event) | InProgress(event) | Completed(event) | Failed(event)
            | Incomplete(event) | Queued(event) => event.sequence_number,
            OutputItemAdded(event) | OutputItemDone(event) => event.sequence_number,
            ContentPartAdded(event) | ContentPartDone(event) => event.sequence_number,
            OutputTextDelta(event) | RefusalDelta(event) | ReasoningTextDelta(event) => {
                event.sequence_number
            }
            OutputTextDone(event) | ReasoningTextDone(event) => event.sequence_number,
            RefusalDone(event) => event.sequence_number,
            FunctionArgumentsDelta(event)
            | McpArgumentsDelta(event)
            | CodeInterpreterCodeDelta(event)
            | CustomToolInputDelta(event) => event.sequence_number,
            FunctionArgumentsDone(event) | McpArgumentsDone(event) => event.sequence_number,
            FileSearchInProgress(event)
            | FileSearchSearching(event)
            | FileSearchCompleted(event)
            | WebSearchInProgress(event)
            | WebSearchSearching(event)
            | WebSearchCompleted(event)
            | ImageGenerationCompleted(event)
            | ImageGenerationGenerating(event)
            | ImageGenerationInProgress(event)
            | McpCallCompleted(event)
            | McpCallFailed(event)
            | McpCallInProgress(event)
            | McpListToolsCompleted(event)
            | McpListToolsFailed(event)
            | McpListToolsInProgress(event)
            | CodeInterpreterInProgress(event)
            | CodeInterpreterInterpreting(event)
            | CodeInterpreterCompleted(event)
            | CompactionCompacting(event) => event.sequence_number,
            ReasoningSummaryPartAdded(event) | ReasoningSummaryPartDone(event) => {
                event.sequence_number
            }
            ReasoningSummaryTextDelta(event) => event.sequence_number,
            ReasoningSummaryTextDone(event) => event.sequence_number,
            ImageGenerationPartial(event) => event.sequence_number,
            CodeInterpreterCodeDone(event) => event.sequence_number,
            AnnotationAdded(event) => event.sequence_number,
            CustomToolInputDone(event) => event.sequence_number,
            Error(event) => event.sequence_number,
            AudioDelta(event) | AudioTranscriptDelta(event) => event.sequence_number,
            AudioDone(event) | AudioTranscriptDone(event) => event.sequence_number,
            ShellCommandAdded(event) | ShellCommandDone(event) => event.sequence_number,
            ShellCommandDelta(event) => event.sequence_number,
            ShellOutputDelta(event) => event.sequence_number,
            ShellOutputDone(event) => event.sequence_number,
        }
    }
}

/// 响应生命周期事件使用完整的类型化快照。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Lifecycle {
    /// 本事件携带的响应快照。
    pub response: Box<Response>,
    /// 事件顺序号。
    pub sequence_number: u64,
    /// 保留未声明的事件字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 输出项开始或完成的载体。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ItemEvent {
    /// 原生函数、消息或服务端输出项。
    pub item: OutputItem,
    /// 输出项在 output 中的索引。
    pub output_index: u64,
    /// 事件顺序号。
    pub sequence_number: u64,
    /// 保留未声明的事件字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 内容块开始或完成的载体。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContentEvent {
    /// 内容块在输出项中的索引。
    pub content_index: u64,
    /// 输出项 ID。
    pub item_id: String,
    /// 输出项在 output 中的索引。
    pub output_index: u64,
    /// 文本、拒绝或思考内容块。
    pub part: ContentPart,
    /// 事件顺序号。
    pub sequence_number: u64,
    /// 保留未声明的事件字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 文本、拒绝和思考正文共用字符串增量的位置。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContentDelta {
    /// 内容块在输出项中的索引。
    pub content_index: u64,
    /// 当前事件追加的字符串，不能假设是完整 JSON。
    pub delta: String,
    /// 输出项 ID。
    pub item_id: String,
    /// 词元对数概率的叶子列表；缺失和 null 分开保留。
    #[serde(default, skip_serializing_if = "O::is_missing")]
    pub logprobs: O<Vec<Value>>,
    /// 输出项在 output 中的索引。
    pub output_index: u64,
    /// 事件顺序号。
    pub sequence_number: u64,
    /// 分片长度混淆字符，不属于输出正文。
    #[serde(default, skip_serializing_if = "O::is_missing")]
    pub obfuscation: O<String>,
    /// 保留未声明的事件字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 完整文本事件，不能再次追加已发送的增量。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextDone {
    /// 内容块在输出项中的索引。
    pub content_index: u64,
    /// 输出项 ID。
    pub item_id: String,
    /// 词元对数概率的叶子列表；缺失和 null 分开保留。
    #[serde(default, skip_serializing_if = "O::is_missing")]
    pub logprobs: O<Vec<Value>>,
    /// 输出项在 output 中的索引。
    pub output_index: u64,
    /// 事件顺序号。
    pub sequence_number: u64,
    /// 已完成的全部文本。
    pub text: String,
    /// 保留未声明的事件字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 完整拒绝说明。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RefusalDone {
    /// 内容块在输出项中的索引。
    pub content_index: u64,
    /// 输出项 ID。
    pub item_id: String,
    /// 输出项在 output 中的索引。
    pub output_index: u64,
    /// 完整拒绝说明。
    pub refusal: String,
    /// 事件顺序号。
    pub sequence_number: u64,
    /// 保留未声明的事件字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 函数、MCP、代码及自定义工具共用项级字符串增量。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ItemDelta {
    /// 当前事件追加的字符串，不能假设是完整 JSON。
    pub delta: String,
    /// 输出项 ID。
    pub item_id: String,
    /// 输出项在 output 中的索引。
    pub output_index: u64,
    /// 事件顺序号。
    pub sequence_number: u64,
    /// 分片长度混淆字符，不属于输出正文。
    #[serde(default, skip_serializing_if = "O::is_missing")]
    pub obfuscation: O<String>,
    /// 保留未声明的事件字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 函数或 MCP 的完整参数文本。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ArgumentsDone {
    /// 完整参数字符串；是否为 JSON 由工具类型决定。
    pub arguments: String,
    /// 输出项 ID。
    pub item_id: String,
    /// 输出项在 output 中的索引。
    pub output_index: u64,
    /// 事件顺序号。
    pub sequence_number: u64,
    /// 保留未声明的事件字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 服务端工具进度，不表示客户端需要执行函数。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ItemStatus {
    /// 输出项 ID。
    pub item_id: String,
    /// 输出项在 output 中的索引。
    pub output_index: u64,
    /// 事件顺序号。
    pub sequence_number: u64,
    /// 保留未声明的事件字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 思考摘要块的开始或完成。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SummaryPart {
    /// 输出项 ID。
    pub item_id: String,
    /// 输出项在 output 中的索引。
    pub output_index: u64,
    /// 摘要内容块。
    pub part: ReasoningPart,
    /// 事件顺序号。
    pub sequence_number: u64,
    /// 思考摘要块的索引。
    pub summary_index: u64,
    /// 保留未声明的事件字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 思考摘要的字符串增量。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SummaryDelta {
    /// 当前事件追加的字符串，不能假设是完整 JSON。
    pub delta: String,
    /// 输出项 ID。
    pub item_id: String,
    /// 输出项在 output 中的索引。
    pub output_index: u64,
    /// 事件顺序号。
    pub sequence_number: u64,
    /// 思考摘要块的索引。
    pub summary_index: u64,
    /// 分片长度混淆字符，不属于输出正文。
    #[serde(default, skip_serializing_if = "O::is_missing")]
    pub obfuscation: O<String>,
    /// 保留未声明的事件字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 完整思考摘要。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SummaryDone {
    /// 输出项 ID。
    pub item_id: String,
    /// 输出项在 output 中的索引。
    pub output_index: u64,
    /// 事件顺序号。
    pub sequence_number: u64,
    /// 思考摘要块的索引。
    pub summary_index: u64,
    /// 已完成的全部文本。
    pub text: String,
    /// 保留未声明的事件字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 图片预览载体，不能作为文本或函数参数拼接。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PartialImage {
    /// 输出项 ID。
    pub item_id: String,
    /// 输出项在 output 中的索引。
    pub output_index: u64,
    /// 本次预览图片的 Base64 数据。
    pub partial_image_b64: String,
    /// 当前预览图片索引。
    pub partial_image_index: u64,
    /// 事件顺序号。
    pub sequence_number: u64,
    /// 保留未声明的事件字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 服务端代码生成完成。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CodeDone {
    /// 完整待执行代码。
    pub code: String,
    /// 输出项 ID。
    pub item_id: String,
    /// 输出项在 output 中的索引。
    pub output_index: u64,
    /// 事件顺序号。
    pub sequence_number: u64,
    /// 保留未声明的事件字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 内容块中的引用标注。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnnotationAdded {
    /// 引用标注叶子对象，允许明确的 null。
    pub annotation: O<Value>,
    /// 引用在标注数组中的索引。
    pub annotation_index: u64,
    /// 内容块在输出项中的索引。
    pub content_index: u64,
    /// 输出项 ID。
    pub item_id: String,
    /// 输出项在 output 中的索引。
    pub output_index: u64,
    /// 事件顺序号。
    pub sequence_number: u64,
    /// 保留未声明的事件字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 自定义工具的完整输入。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InputDone {
    /// 完整自定义工具输入。
    pub input: String,
    /// 输出项 ID。
    pub item_id: String,
    /// 输出项在 output 中的索引。
    pub output_index: u64,
    /// 事件顺序号。
    pub sequence_number: u64,
    /// 保留未声明的事件字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// HTTP 发头后的流内错误。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ErrorEvent {
    /// 错误代码，允许 null。
    pub code: O<String>,
    /// 错误说明；日志不得直接打印该字段。
    pub message: String,
    /// 错误关联的请求参数，允许 null。
    pub param: O<String>,
    /// 事件顺序号。
    pub sequence_number: u64,
    /// 保留未声明的事件字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 音频或转录增量，不推测编码格式。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioDelta {
    /// 当前事件追加的字符串，不能假设是完整 JSON。
    pub delta: String,
    /// 事件顺序号。
    pub sequence_number: u64,
    /// 所属响应 ID；兼容未携带该字段的事件。
    #[serde(default, skip_serializing_if = "O::is_missing")]
    pub response_id: O<String>,
    /// 保留未声明的事件字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 音频或转录结束，未隐含完整响应终止。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioDone {
    /// 事件顺序号。
    pub sequence_number: u64,
    /// 所属响应 ID；兼容未携带该字段的事件。
    #[serde(default, skip_serializing_if = "O::is_missing")]
    pub response_id: O<String>,
    /// 保留未声明的事件字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 服务端 shell 命令的新增或完整载体。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShellCommand {
    /// 完整命令文本。
    pub command: String,
    /// 当前命令在调用中的索引。
    pub command_index: u64,
    /// 输出项在 output 中的索引。
    pub output_index: u64,
    /// 事件顺序号。
    pub sequence_number: u64,
    /// 保留未声明的事件字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 服务端 shell 命令增量。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShellCommandDelta {
    /// 当前命令在调用中的索引。
    pub command_index: u64,
    /// 当前事件追加的字符串，不能假设是完整 JSON。
    pub delta: String,
    /// 输出项在 output 中的索引。
    pub output_index: u64,
    /// 事件顺序号。
    pub sequence_number: u64,
    /// 分片长度混淆字符，不属于输出正文。
    #[serde(default, skip_serializing_if = "O::is_missing")]
    pub obfuscation: O<String>,
    /// 保留未声明的事件字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// stdout/stderr 的局部增量。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShellOutputDelta {
    /// 当前命令在调用中的索引。
    pub command_index: u64,
    /// 标准输出及错误输出的增量对象。
    pub delta: ShellText,
    /// 输出项 ID。
    pub item_id: String,
    /// 输出项在 output 中的索引。
    pub output_index: u64,
    /// 事件顺序号。
    pub sequence_number: u64,
    /// 保留未声明的事件字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 一个服务端命令的完整执行结果。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShellOutputDone {
    /// 当前命令在调用中的索引。
    pub command_index: u64,
    /// 输出项 ID。
    pub item_id: String,
    /// 按生成顺序排列的输出块。
    pub output: Vec<ShellOutput>,
    /// 输出项在 output 中的索引。
    pub output_index: u64,
    /// 事件顺序号。
    pub sequence_number: u64,
    /// 保留未声明的事件字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 标准输出与错误输出各自独立追加。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShellText {
    /// 新增错误输出。
    #[serde(default, skip_serializing_if = "O::is_missing")]
    pub stderr: O<String>,
    /// 新增标准输出。
    #[serde(default, skip_serializing_if = "O::is_missing")]
    pub stdout: O<String>,
    /// 保留未声明的事件字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 服务端执行的单个输出块。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShellOutput {
    /// 退出码或超时状态。
    pub outcome: ShellOutcome,
    /// 完整错误输出。
    pub stderr: String,
    /// 完整标准输出。
    pub stdout: String,
    /// 输出创建者标识，属于 Provider 状态。
    #[serde(default, skip_serializing_if = "O::is_missing")]
    pub created_by: O<String>,
    /// 保留未声明的事件字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 输出文本和拒绝复用非流式内容块，思考块单独声明。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ContentPart {
    /// 普通输出文本或拒绝。
    Output(super::message::ContentPart),
    /// 思考正文或摘要。
    Reasoning(ReasoningPart),
}
/// 思考正文与摘要具有不同类型标签。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReasoningPart {
    /// 原生思考正文。
    ReasoningText {
        /// 思考文本。
        text: String,
        /// 未声明的内容字段。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    /// 可见思考摘要。
    SummaryText {
        /// 摘要文本。
        text: String,
        /// 未声明的内容字段。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
}
/// 服务端执行状态，不依赖未声明的 JSON 对象判定。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ShellOutcome {
    /// 执行超时。
    Timeout {
        /// 未声明的状态字段。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    /// 命令已经退出。
    Exit {
        /// 原生退出码。
        exit_code: i64,
        /// 未声明的状态字段。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
}

#[cfg(test)]
mod tests {
    use super::{Event, KnownEvent};
    use serde_json::json;
    #[test]
    fn typed_events_keep_distinct_payloads_and_unknown_extensions() {
        for source in [
            json!({"type":"response.output_text.delta","content_index":0,"delta":"Hi","item_id":"m","output_index":0,"sequence_number":4,"logprobs":null}),
            json!({"type":"response.function_call_arguments.delta","delta":"{","item_id":"f","output_index":1,"sequence_number":5}),
            json!({"type":"response.content_part.added","content_index":0,"item_id":"r","output_index":0,"part":{"type":"reasoning_text","text":"plan"},"sequence_number":1}),
            json!({"type":"response.content_part.added","content_index":0,"item_id":"m","output_index":0,"part":{"type":"output_text","text":"Hi","annotations":[],"logprobs":null},"sequence_number":2}),
            json!({"type":"response.audio.delta","delta":"YQ==","sequence_number":6}),
            json!({"type":"response.shell_call_output_content.delta","command_index":0,"delta":{"stdout":"OK"},"item_id":"s","output_index":2,"sequence_number":7}),
            json!({"type":"error","code":null,"message":"busy","param":null,"sequence_number":8}),
            json!({"type":"vendor_event","payload":{"future":true}}),
        ] {
            let event: Event =
                serde_json::from_slice(&serde_json::to_vec(&source).unwrap()).unwrap();
            assert_eq!(serde_json::to_value(&event).unwrap(), source);
            if source["type"] == "response.shell_call_output_content.delta" {
                assert!(
                    matches!(event, Event::Known(event) if matches!(*event, KnownEvent::ShellOutputDelta(_)))
                );
            }
        }
    }
    #[test]
    fn known_malformed_events_are_not_opaque_extensions() {
        for source in [
            json!({"type":"response.output_text.delta","delta":"Hi"}),
            json!({"type":"response.function_call_arguments.delta","delta":{},"item_id":"f","output_index":0,"sequence_number":0}),
            json!({"type":"response.created","response":{},"sequence_number":0}),
            json!({"type":"response.shell_call_output_content.delta","delta":"not-an-object","command_index":0,"item_id":"s","output_index":0,"sequence_number":0}),
        ] {
            assert!(serde_json::from_value::<Event>(source).is_err());
        }
    }
}
