//! Messages API 返回的顶层 Message 及其内容块。
//! 参考 API：https://platform.claude.com/docs/en/api/messages/create

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

pub use crate::protocol::common::{AssistantRole, MessageType};
use crate::protocol::optional_nullable::OptionalNullable;

/// 非流式 Messages API 的完整响应消息。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    /// 对象类型，固定为 `message`。
    pub r#type: MessageType,
    /// 本次消息的唯一 ID。
    pub id: String,
    /// 容器工具的状态；未使用容器时可为 `null`。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub container: OptionalNullable<Value>,
    /// 按生成顺序排列的输出内容块。
    pub content: Vec<ContentBlock>,
    /// 提示缓存等诊断信息。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub diagnostics: OptionalNullable<Value>,
    /// 实际生成此消息的模型 ID。
    pub model: String,
    /// 生成者角色，固定为 `assistant`。
    pub role: AssistantRole,
    /// 拒绝等停止原因的结构化详情。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub stop_details: OptionalNullable<Value>,
    /// 停止原因；生成尚未完成时可为 `null`。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub stop_reason: OptionalNullable<String>,
    /// 命中的自定义停止序列。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub stop_sequence: OptionalNullable<String>,
    /// 输入、输出及缓存 token 用量；细项暂以 JSON 保留。
    pub usage: Value,
    /// 容器、诊断等新增响应字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 已声明的输出块与其他服务端工具块。
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ContentBlock {
    /// 文字、思考或客户端工具调用。
    Known(KnownContentBlock),
    /// 未建模的服务端工具、搜索等内容块。
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
            "text" | "thinking" | "redacted_thinking" | "tool_use"
        ) {
            KnownContentBlock::deserialize(value)
                .map(Self::Known)
                .map_err(serde::de::Error::custom)
        } else {
            Ok(Self::Other(object.clone()))
        }
    }
}

/// 首批可解释的模型输出内容块。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum KnownContentBlock {
    /// 模型输出的文字。
    Text {
        /// 输出文本。
        text: String,
        /// 文本引用标注；可能为 `null` 或省略。
        #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
        citations: OptionalNullable<Vec<Value>>,
        /// 保留文本块扩展字段。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    /// 可回传给模型的思考内容。
    Thinking {
        /// 用于验证思考块的签名。
        signature: String,
        /// 模型输出的思考文本。
        thinking: String,
        /// 保留思考块扩展字段。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    /// 安全系统遮蔽后的不透明思考内容。
    RedactedThinking {
        /// 加密后的不透明数据。
        data: String,
        /// 保留遮蔽思考块扩展字段。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    /// 模型发起的客户端工具调用。
    ToolUse {
        /// 工具调用 ID。
        id: String,
        /// 工具调用来源，直接调用或由服务端工具产生。
        #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
        caller: OptionalNullable<Value>,
        /// 结构化 JSON 参数对象。
        input: Map<String, Value>,
        /// 工具名称。
        name: String,
        /// 保留工具块扩展字段。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
}

#[cfg(test)]
mod tests {
    use super::{ContentBlock, KnownContentBlock, Message};
    use serde_json::json;

    #[test]
    fn response_message_round_trip() {
        let source = json!({"id":"msg_1","content":[{"type":"text","text":"hello","citations":null},{"type":"thinking","signature":"sig","thinking":"plan"},{"type":"tool_use","id":"toolu_1","name":"lookup","input":{"q":1}},{"type":"server_tool_use","id":"srv_1","name":"search","input":{}}],"model":"claude-example","role":"assistant","stop_reason":"tool_use","stop_sequence":null,"type":"message","usage":{"input_tokens":3,"output_tokens":4},"container":{"id":"ctr_1"}});
        let message: Message = serde_json::from_value(source.clone()).unwrap();
        assert!(matches!(
            message.content[0],
            ContentBlock::Known(KnownContentBlock::Text { .. })
        ));
        assert!(matches!(message.content[3], ContentBlock::Other(_)));
        assert_eq!(serde_json::to_value(message).unwrap(), source);
    }
}
