//! 对话历史先组成 IR，再通过统一 Codec 构造当前选择的协议类型。
use super::history::{Conversation, Selection};
use llmproxy_core::{
    adapter::protocol_codec::{Conversion, EncodeMode, ProtocolCodec, RequestTarget},
    protocol::Request,
};
use reqwest::RequestBuilder;

/// 请求意图属于本轮，切换后的模型、输出模式不会修改已保存历史。
pub(super) fn request_body(
    selection: &Selection,
    alias: &str,
    history: &Conversation,
    stream: bool,
) -> Result<Conversion<Request>, String> {
    let prepared = history.request(selection);
    let mut ir = prepared.body;
    ir.model = Some(alias.into());
    ir.generation.stream = stream;
    ir.generation.max_output_tokens = Some(2048);
    let mut converted = selection
        .protocol
        .encode_request(
            &ir,
            &RequestTarget {
                model: alias,
                max_output_tokens: None,
            },
            EncodeMode::Rebuild,
        )
        .map_err(|_| "当前模型无法处理这段对话历史，请选择兼容模型或新建会话".to_owned())?;
    converted.warnings.splice(0..0, prepared.warnings);
    Ok(converted)
}

/// 在 HTTP 边界序列化具体协议结构，避免将统一载体的枚举标签写入请求正文。
pub(super) fn with_request_body(builder: RequestBuilder, body: &Request) -> RequestBuilder {
    builder.json(&llmproxy_core::protocol::wire::request(body))
}
