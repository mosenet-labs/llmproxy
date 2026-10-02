//! Messages 请求中 `messages` 数组的原始协议结构。
//! 参考 API：https://platform.claude.com/docs/en/api/http/messages
//! 消息字段定义：https://github.com/anthropics/anthropic-sdk-python/blob/main/src/anthropic/types/message_param.py
//! 内容块字段定义：https://github.com/anthropics/anthropic-sdk-python/blob/main/src/anthropic/types/content_block_param.py

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

use super::cache::CacheControl;
use crate::protocol::optional_nullable::OptionalNullable;

/// `messages` 数组中的一条输入消息。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    /// 消息正文，可以是文本字符串或内容块数组。
    pub content: Content,
    /// 消息作者；`system` 消息仅在支持会话中途系统消息的模型上可用。
    pub role: Role,
    /// 保留当前未声明的扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 消息作者的角色。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// 用户输入或工具执行结果。
    User,
    /// 模型回复或工具调用。
    Assistant,
    /// 会话中途的系统指令，适用性由上游模型决定。
    System,
}

/// 消息正文的两种原始 JSON 形状。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Content {
    /// 单段文本，等价于只含一个文本块的数组。
    Text(String),
    /// 按顺序排列的内容块。
    Parts(Vec<ContentBlock>),
}

/// 请求中的内容块；未声明的块类型保留原始对象。
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ContentBlock {
    /// 已声明的常用内容块。
    Known(KnownContentBlock),
    /// 其他内容块，例如服务端工具结果，保留其全部字段。
    Other(Map<String, Value>),
}

impl<'de> Deserialize<'de> for ContentBlock {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        let Value::Object(object) = &value else {
            return Err(serde::de::Error::custom("内容块必须是对象"));
        };
        let Some(Value::String(kind)) = object.get("type") else {
            return Err(serde::de::Error::custom("内容块缺少字符串类型字段"));
        };

        if matches!(
            kind.as_str(),
            "text"
                | "image"
                | "document"
                | "thinking"
                | "redacted_thinking"
                | "tool_use"
                | "tool_result"
        ) {
            KnownContentBlock::deserialize(value)
                .map(Self::Known)
                .map_err(serde::de::Error::custom)
        } else {
            Ok(Self::Other(object.clone()))
        }
    }
}

/// 当前声明字段的内容块，按 `type` 区分。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum KnownContentBlock {
    /// `type: "text"` 的文本块。
    Text {
        /// 文本内容。
        text: String,
        /// 可选的提示缓存断点。
        #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
        cache_control: OptionalNullable<CacheControl>,
        /// 文本引用标注。
        #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
        citations: OptionalNullable<Vec<Value>>,
        /// 保留当前未声明的扩展字段。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    /// `type: "image"` 的图片块。
    Image {
        /// 图片来源，例如 Base64、URL 或文件 ID；来源变体暂以 JSON 保留。
        source: Value,
        /// 可选的提示缓存断点。
        #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
        cache_control: OptionalNullable<CacheControl>,
        /// 服务端对图片应用的变换配置。
        #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
        transformations: OptionalNullable<Value>,
        /// 保留当前未声明的扩展字段。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    /// `type: "document"` 的文档块。
    Document {
        /// 文档来源，例如 PDF、纯文本、URL 或文件 ID；来源变体暂以 JSON 保留。
        source: Value,
        /// 可选的提示缓存断点。
        #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
        cache_control: OptionalNullable<CacheControl>,
        /// 文档引用的启用配置。
        #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
        citations: OptionalNullable<Value>,
        /// 附加给文档的上下文说明。
        #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
        context: OptionalNullable<String>,
        /// 文档标题。
        #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
        title: OptionalNullable<String>,
        /// 保留当前未声明的扩展字段。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    /// `type: "thinking"` 的思考块，回传时应保持原样和顺序。
    Thinking {
        /// 验证此思考块来自模型的签名。
        signature: String,
        /// 模型返回的思考文本。
        thinking: String,
        /// 保留当前未声明的扩展字段。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    /// `type: "redacted_thinking"` 的加密思考块。
    RedactedThinking {
        /// 模型返回的不透明加密数据，回传时应保持原样。
        data: String,
        /// 保留当前未声明的扩展字段。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    /// `type: "tool_use"` 的客户端工具调用。
    ToolUse {
        /// 工具调用的唯一 ID，用于匹配工具结果。
        id: String,
        /// 传给工具的 JSON 对象。
        input: Map<String, Value>,
        /// 被调用工具的名称。
        name: String,
        /// 可选的提示缓存断点。
        #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
        cache_control: OptionalNullable<CacheControl>,
        /// 调用来源，例如直接调用或服务端工具调用。
        #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
        caller: OptionalNullable<Value>,
        /// 所属工具集的名称。
        #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
        toolset_name: OptionalNullable<String>,
        /// 保留当前未声明的扩展字段。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    /// `type: "tool_result"` 的客户端工具执行结果。
    ToolResult {
        /// 此结果所对应的工具调用 ID。
        tool_use_id: String,
        /// 可选的提示缓存断点。
        #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
        cache_control: OptionalNullable<CacheControl>,
        /// 结果正文，可以是字符串或内容块数组。
        #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
        content: OptionalNullable<ToolResultContent>,
        /// 工具执行是否出错。
        #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
        is_error: OptionalNullable<bool>,
        /// 与对应工具调用相同的工具集名称。
        #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
        toolset_name: OptionalNullable<String>,
        /// 保留当前未声明的扩展字段。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
}

/// 工具结果正文的两种原始 JSON 形状。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolResultContent {
    /// 纯文本结果。
    Text(String),
    /// 工具返回的内容块；具体块字段暂以 JSON 保留。
    Parts(Vec<Value>),
}

#[cfg(test)]
mod tests {
    use super::{Content, ContentBlock, KnownContentBlock, Message, Role, ToolResultContent};
    use serde_json::json;

    #[test]
    fn request_messages_round_trip() {
        let source = json!([
            {"role": "user", "content": "Hello"},
            {"role": "assistant", "content": [
                {"type": "text", "text": "I'll check", "cache_control": {"type": "ephemeral"}},
                {"type": "thinking", "signature": "sig", "thinking": "reason"},
                {"type": "redacted_thinking", "data": "encrypted"},
                {"type": "tool_use", "id": "toolu_1", "name": "lookup", "input": {"query": "x"}}
            ]},
            {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "toolu_1", "content": "done", "is_error": false},
                {"type": "image", "source": {"type": "url", "url": "https://example.com/a.png"}},
                {"type": "document", "source": {"type": "file", "file_id": "file_1"}},
                {"type": "search_result", "source": "web", "title": "Example"}
            ], "provider_extension": true},
            {"role": "system", "content": "Use this instruction from now on"}
        ]);
        let messages: Vec<Message> = serde_json::from_value(source.clone()).unwrap();

        assert!(matches!(messages[0].content, Content::Text(_)));
        assert_eq!(messages[3].role, Role::System);
        assert!(matches!(&messages[1].content, Content::Parts(parts)
            if matches!(&parts[3], ContentBlock::Known(KnownContentBlock::ToolUse { .. }))));
        assert!(matches!(&messages[2].content, Content::Parts(parts)
            if matches!(&parts[0], ContentBlock::Known(KnownContentBlock::ToolResult {
                content: super::OptionalNullable::Value(ToolResultContent::Text(_)), ..
            })) && matches!(&parts[3], ContentBlock::Other(_))));
        assert_eq!(serde_json::to_value(messages).unwrap(), source);
    }

    #[test]
    fn missing_required_fields_are_rejected() {
        assert!(serde_json::from_value::<Message>(json!({"content": "Hi"})).is_err());
        assert!(serde_json::from_value::<Message>(json!({"role": "user"})).is_err());
        assert!(serde_json::from_value::<Message>(json!({
            "role": "assistant", "content": [{"type": "tool_use", "name": "lookup", "input": {}}]
        }))
        .is_err());
    }
}
