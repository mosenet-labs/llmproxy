//! 在完整 JSON 报文中定位消息节点，复用各协议的消息适配器。

use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::{
    ir::{request::Message as RequestMessage, response::Message as ResponseMessage},
    protocol::{Protocol, chat, gemini, messages, responses},
};

use super::{Error, Result, request, response};

/// 消息 IR 与其在原 JSON 报文中的位置；暂只允许逐条编辑，不改变消息数量。
pub struct MessageBatch<M> {
    /// 按原报文顺序排列、可供调用方编辑的消息 IR。
    pub messages: Vec<M>,
    original: Vec<M>,
    paths: Vec<String>,
}

/// 协议消息的投影契约；完整报文参数仍由调用方以 `Value` 保存。
pub trait MessageCodec {
    /// 从请求外壳提取消息；没有适用的消息数组时返回 `None`。
    fn decode_request_messages(&self, body: &Value)
    -> Result<Option<MessageBatch<RequestMessage>>>;
    /// 将请求消息写回原位置；未发生 IR 编辑时返回 `false` 并保持外壳不变。
    fn encode_request_messages(
        &self,
        body: &mut Value,
        batch: MessageBatch<RequestMessage>,
    ) -> Result<bool>;
    /// 从非流式响应外壳提取消息，保留其他输出项和候选字段。
    fn decode_response_messages(
        &self,
        body: &Value,
    ) -> Result<Option<MessageBatch<ResponseMessage>>>;
    /// 将响应消息写回原位置；未发生 IR 编辑时返回 `false`。
    fn encode_response_messages(
        &self,
        body: &mut Value,
        batch: MessageBatch<ResponseMessage>,
    ) -> Result<bool>;
}

/// 返回顶层数组各元素的 JSON Pointer；字段缺失或非数组时返回 `None`。
fn array_paths(body: &Value, key: &str) -> Option<Vec<String>> {
    Some(
        body.get(key)?
            .as_array()?
            .iter()
            .enumerate()
            .map(|(index, _)| format!("/{key}/{index}"))
            .collect(),
    )
}

/// 选择请求消息位置；Responses 中的独立工具项不属于 message。
fn request_paths(protocol: Protocol, body: &Value) -> Option<Vec<String>> {
    let key = match protocol {
        Protocol::OpenAiChat | Protocol::AnthropicMessages => "messages",
        Protocol::OpenAiResponses => "input",
        Protocol::Gemini => "contents",
    };
    let paths = array_paths(body, key)?;
    if protocol != Protocol::OpenAiResponses {
        return Some(paths);
    }
    // Responses 的 input 可混排函数调用、结果等非消息输入项。
    Some(
        paths
            .into_iter()
            .filter(|path| {
                let item = body.pointer(path).expect("从现有数组生成的消息位置");
                match item.get("type").and_then(Value::as_str) {
                    Some("message") => true,
                    Some(_) => false,
                    None => item.get("role").is_some() && item.get("content").is_some(),
                }
            })
            .collect(),
    )
}

/// 选择响应消息位置；错误对象和不含消息的输出不参与转换。
fn response_paths(protocol: Protocol, body: &Value) -> Option<Vec<String>> {
    match protocol {
        Protocol::OpenAiChat => Some(
            array_paths(body, "choices")?
                .into_iter()
                .map(|path| format!("{path}/message"))
                .filter(|path| body.pointer(path).is_some_and(Value::is_object))
                .collect(),
        ),
        Protocol::OpenAiResponses => Some(
            array_paths(body, "output")?
                .into_iter()
                .filter(|path| {
                    body.pointer(path)
                        .is_some_and(|item| item["type"] == "message")
                })
                .collect(),
        ),
        Protocol::AnthropicMessages => {
            (body.get("type")?.as_str()? == "message").then(|| vec![String::new()])
        }
        Protocol::Gemini => Some(
            array_paths(body, "candidates")?
                .into_iter()
                .map(|path| format!("{path}/content"))
                .filter(|path| body.pointer(path).is_some_and(Value::is_object))
                .collect(),
        ),
    }
}

/// 将选中的原协议 DTO 解码为 IR，并记住回填位置和初始状态。
fn decode_batch<D: DeserializeOwned, M: Clone>(
    body: &Value,
    paths: Vec<String>,
    decode: impl FnOnce(&[D]) -> Result<Vec<M>>,
) -> Result<MessageBatch<M>> {
    let raw = paths
        .iter()
        .map(|path| {
            body.pointer(path)
                .cloned()
                .ok_or_else(|| Error::Invalid(format!("消息位置 {path} 不存在")))
        })
        .collect::<Result<Vec<_>>>()?;
    let raw: Vec<D> = serde_json::from_value(Value::Array(raw))?;
    let messages = decode(&raw)?;
    if messages.len() != paths.len() {
        return Err(Error::Invalid("消息适配器改变了消息数量".into()));
    }
    Ok(MessageBatch {
        original: messages.clone(),
        messages,
        paths,
    })
}

/// 只回填已编辑的消息；消息数量变化或位置失效时拒绝写入。
fn encode_batch<D: Serialize, M: PartialEq>(
    body: &mut Value,
    batch: MessageBatch<M>,
    encode: impl FnOnce(&[M]) -> Result<Vec<D>>,
) -> Result<bool> {
    let MessageBatch {
        messages,
        original,
        paths,
    } = batch;
    if messages.len() != paths.len() {
        return Err(Error::Unsupported("当前不支持改变消息数量".into()));
    }
    if messages == original {
        return Ok(false);
    }
    let raw = encode(&messages)?;
    if raw.len() != paths.len() || paths.iter().any(|path| body.pointer(path).is_none()) {
        return Err(Error::Invalid("消息位置与编码结果不一致".into()));
    }
    let raw = raw
        .into_iter()
        .map(serde_json::to_value)
        .collect::<serde_json::Result<Vec<_>>>()?;
    for ((path, value), (message, original)) in paths
        .iter()
        .zip(raw)
        .zip(messages.iter().zip(original.iter()))
    {
        if message != original {
            *body.pointer_mut(path).expect("已验证消息位置") = value;
        }
    }
    Ok(true)
}

impl MessageCodec for Protocol {
    fn decode_request_messages(
        &self,
        body: &Value,
    ) -> Result<Option<MessageBatch<RequestMessage>>> {
        let Some(paths) = request_paths(*self, body) else {
            return Ok(None);
        };
        Ok(Some(match self {
            Self::OpenAiChat => decode_batch::<chat::request::message::Message, _>(
                body,
                paths,
                request::decode_chat,
            )?,
            Self::OpenAiResponses => decode_batch::<responses::request::message::Message, _>(
                body,
                paths,
                request::decode_responses,
            )?,
            Self::AnthropicMessages => decode_batch::<messages::request::message::Message, _>(
                body,
                paths,
                request::decode_messages,
            )?,
            Self::Gemini => decode_batch::<gemini::request::message::Message, _>(
                body,
                paths,
                request::decode_gemini,
            )?,
        }))
    }

    fn encode_request_messages(
        &self,
        body: &mut Value,
        batch: MessageBatch<RequestMessage>,
    ) -> Result<bool> {
        match self {
            Self::OpenAiChat => encode_batch::<chat::request::message::Message, _>(
                body,
                batch,
                request::encode_chat,
            ),
            Self::OpenAiResponses => encode_batch::<responses::request::message::Message, _>(
                body,
                batch,
                request::encode_responses,
            ),
            Self::AnthropicMessages => encode_batch::<messages::request::message::Message, _>(
                body,
                batch,
                request::encode_messages,
            ),
            Self::Gemini => encode_batch::<gemini::request::message::Message, _>(
                body,
                batch,
                request::encode_gemini,
            ),
        }
    }

    fn decode_response_messages(
        &self,
        body: &Value,
    ) -> Result<Option<MessageBatch<ResponseMessage>>> {
        let Some(paths) = response_paths(*self, body) else {
            return Ok(None);
        };
        Ok(Some(match self {
            Self::OpenAiChat => decode_batch::<chat::response::message::Message, _>(
                body,
                paths,
                response::decode_chat,
            )?,
            Self::OpenAiResponses => decode_batch::<responses::response::message::Message, _>(
                body,
                paths,
                response::decode_responses,
            )?,
            Self::AnthropicMessages => decode_batch::<messages::response::message::Message, _>(
                body,
                paths,
                response::decode_messages,
            )?,
            Self::Gemini => decode_batch::<gemini::response::message::Message, _>(
                body,
                paths,
                response::decode_gemini,
            )?,
        }))
    }

    fn encode_response_messages(
        &self,
        body: &mut Value,
        batch: MessageBatch<ResponseMessage>,
    ) -> Result<bool> {
        match self {
            Self::OpenAiChat => encode_batch::<chat::response::message::Message, _>(
                body,
                batch,
                response::encode_chat,
            ),
            Self::OpenAiResponses => encode_batch::<responses::response::message::Message, _>(
                body,
                batch,
                response::encode_responses,
            ),
            Self::AnthropicMessages => encode_batch::<messages::response::message::Message, _>(
                body,
                batch,
                response::encode_messages,
            ),
            Self::Gemini => encode_batch::<gemini::response::message::Message, _>(
                body,
                batch,
                response::encode_gemini,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::{ir::message::PartKind, protocol::Protocol};

    use super::MessageCodec;

    fn change_first_text(parts: &mut [crate::ir::message::Part]) {
        let PartKind::Text(text) = &mut parts[0].kind else {
            panic!("预期首个内容块为文本");
        };
        *text = "edited".into();
    }

    #[test]
    fn request_messages_keep_envelope_and_unselected_items() {
        let cases: [(Protocol, Value, &str); 4] = [
            (
                Protocol::OpenAiChat,
                json!({"model":"m","messages":[{"role":"user","content":"hi"}],"temperature":0.3}),
                "/messages/0/content",
            ),
            (
                Protocol::OpenAiResponses,
                json!({"model":"m","input":[{"role":"user","content":"hi"},{"type":"function_call","id":"call_1","name":"lookup","arguments":"{}"}],"instructions":"keep"}),
                "/input/0/content",
            ),
            (
                Protocol::AnthropicMessages,
                json!({"model":"m","messages":[{"role":"user","content":"hi"}],"max_tokens":50}),
                "/messages/0/content",
            ),
            (
                Protocol::Gemini,
                json!({"contents":[{"role":"user","parts":[{"text":"hi"}]}],"generationConfig":{"temperature":0.3}}),
                "/contents/0/parts/0/text",
            ),
        ];
        for (protocol, mut body, text_path) in cases {
            let original = body.clone();
            let batch = protocol.decode_request_messages(&body).unwrap().unwrap();
            assert_eq!(batch.messages.len(), 1);
            assert!(!protocol.encode_request_messages(&mut body, batch).unwrap());
            assert_eq!(body, original);

            let mut batch = protocol.decode_request_messages(&body).unwrap().unwrap();
            change_first_text(&mut batch.messages[0].parts);
            assert!(protocol.encode_request_messages(&mut body, batch).unwrap());
            assert_eq!(body.pointer(text_path), Some(&json!("edited")));
            if protocol == Protocol::OpenAiResponses {
                assert_eq!(body["input"][1], original["input"][1]);
            }
        }
    }

    #[test]
    fn response_messages_keep_other_choices_and_output_items() {
        let cases: [(Protocol, Value, &str); 4] = [
            (
                Protocol::OpenAiChat,
                json!({"choices":[{"index":0,"message":{"role":"assistant","content":"hi"}},{"index":1,"message":{"role":"assistant","content":"second"}}],"usage":{"total_tokens":2}}),
                "/choices/0/message/content",
            ),
            (
                Protocol::OpenAiResponses,
                json!({"output":[{"type":"reasoning","id":"r1","summary":[]},{"id":"msg_1","content":[{"type":"output_text","annotations":[],"text":"hi"}],"role":"assistant","status":"completed","type":"message"}],"usage":{"total_tokens":2}}),
                "/output/1/content/0/text",
            ),
            (
                Protocol::AnthropicMessages,
                json!({"type":"message","id":"m1","content":[{"type":"text","text":"hi"}],"model":"claude","role":"assistant","stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":1,"output_tokens":1}}),
                "/content/0/text",
            ),
            (
                Protocol::Gemini,
                json!({"candidates":[{"content":{"role":"model","parts":[{"text":"hi"}]}},{"content":{"role":"model","parts":[{"text":"second"}]}}],"usageMetadata":{"totalTokenCount":2}}),
                "/candidates/0/content/parts/0/text",
            ),
        ];
        for (protocol, mut body, text_path) in cases {
            let original = body.clone();
            let batch = protocol.decode_response_messages(&body).unwrap().unwrap();
            assert!(!protocol.encode_response_messages(&mut body, batch).unwrap());
            assert_eq!(body, original);

            let mut batch = protocol.decode_response_messages(&body).unwrap().unwrap();
            change_first_text(&mut batch.messages[0].parts);
            assert!(protocol.encode_response_messages(&mut body, batch).unwrap());
            assert_eq!(body.pointer(text_path), Some(&json!("edited")));
            if protocol == Protocol::OpenAiResponses {
                assert_eq!(body["output"][0], original["output"][0]);
            }
        }
    }

    #[test]
    fn missing_message_and_count_change_are_explicit() {
        assert!(
            Protocol::OpenAiResponses
                .decode_request_messages(&json!({"input":"plain text"}))
                .unwrap()
                .is_none()
        );
        assert!(
            Protocol::OpenAiChat
                .decode_response_messages(&json!({"error":{"message":"bad"}}))
                .unwrap()
                .is_none()
        );
        let mut body = json!({"messages":[{"role":"user","content":"hi"}]});
        let mut batch = Protocol::OpenAiChat
            .decode_request_messages(&body)
            .unwrap()
            .unwrap();
        batch.messages.clear();
        assert!(
            Protocol::OpenAiChat
                .encode_request_messages(&mut body, batch)
                .is_err()
        );
    }
}
