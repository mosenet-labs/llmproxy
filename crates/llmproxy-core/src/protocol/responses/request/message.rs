//! Responses 请求 `input` 数组中的原始消息结构，不包含独立的工具调用等输入项。
//! 参考 API：https://developers.openai.com/api/reference/resources/responses/methods/create
//! 消息字段定义：https://github.com/openai/openai-python/blob/main/src/openai/types/responses/easy_input_message_param.py
//! 输入项定义：https://github.com/openai/openai-python/blob/main/src/openai/types/responses/response_input_item_param.py

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::protocol::optional_nullable::OptionalNullable;

/// `input` 数组中的消息。工具调用和工具结果是其他输入项类型。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Message {
    /// 可作为历史输入回传的模型输出消息。
    Output(OutputMessage),
    /// 内容必须是输入内容块数组的消息。
    Input(InputMessage),
    /// 内容可以是字符串或输入内容块数组的简化消息。
    Easy(EasyInputMessage),
}

/// 简化输入消息，对应 `EasyInputMessage`。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EasyInputMessage {
    /// 文本、图片或文件输入；可包含先前的 assistant 消息内容。
    pub content: Content<InputPart>,
    /// 消息作者的角色。
    pub role: Role,
    /// assistant 消息的阶段，可标为中间说明或最终回答。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub phase: OptionalNullable<Phase>,
    /// 消息输入项的类型；存在时固定为 `message`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#type: Option<MessageType>,
    /// 保留当前未声明的扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 内容块数组形式的输入消息，对应 `ResponseInputItem.Message`。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InputMessage {
    /// 一组按顺序提供给模型的输入内容块。
    pub content: Vec<InputPart>,
    /// 消息作者的角色。
    pub role: InputRole,
    /// 输入项状态；API 返回输入项时可能填入。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<Status>,
    /// 消息输入项的类型；存在时固定为 `message`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#type: Option<MessageType>,
    /// 保留当前未声明的扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 可回传到请求 `input` 的 assistant 输出消息。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OutputMessage {
    /// 模型输出消息的唯一 ID。
    pub id: String,
    /// 模型输出的文本或拒绝内容块。
    pub content: Vec<OutputPart>,
    /// 输出消息的角色，固定为 `assistant`。
    pub role: AssistantRole,
    /// 消息处理状态。
    pub status: Status,
    /// 输出项类型，固定为 `message`。
    pub r#type: MessageType,
    /// assistant 消息的阶段，可标为中间说明或最终回答。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub phase: OptionalNullable<Phase>,
    /// 保留当前未声明的扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 简化输入消息允许的角色。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
    System,
    Developer,
}

/// 内容块数组形式的输入消息允许的角色。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputRole {
    User,
    System,
    Developer,
}

/// 模型输出消息的固定角色。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssistantRole {
    Assistant,
}

/// 消息输入项或输出项的固定类型。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageType {
    Message,
}

/// assistant 消息在一次响应中的阶段。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Commentary,
    FinalAnswer,
}

/// 输入或输出消息的处理状态。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    InProgress,
    Completed,
    Incomplete,
}

/// 简化消息的内容可以是字符串或内容块数组。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Content<P> {
    /// 纯文本输入。
    Text(String),
    /// 按顺序排列的内容块。
    Parts(Vec<P>),
}

/// 消息中的输入内容块。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InputPart {
    /// `type: "input_text"` 的文本输入。
    InputText {
        /// 提供给模型的文本。
        text: String,
        /// 保留当前未声明的扩展字段，例如提示缓存断点。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    /// `type: "input_image"` 的图片输入。
    InputImage {
        /// 图片细节级别；省略时由 API 选择默认级别。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
        /// 已上传图片文件的 ID。
        #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
        file_id: OptionalNullable<String>,
        /// 图片 URL 或包含 Base64 图片数据的 Data URL。
        #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
        image_url: OptionalNullable<String>,
        /// 保留当前未声明的扩展字段，例如提示缓存断点。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    /// `type: "input_file"` 的文件输入。
    InputFile {
        /// 文件的处理细节级别，可为 `auto`、`low` 或 `high`。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
        /// 直接传入的文件内容。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file_data: Option<String>,
        /// 已上传文件的 ID。
        #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
        file_id: OptionalNullable<String>,
        /// 文件 URL。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file_url: Option<String>,
        /// 文件名。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filename: Option<String>,
        /// 保留当前未声明的扩展字段，例如提示缓存断点。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
}

/// 可回传的 assistant 输出消息中的内容块。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutputPart {
    /// `type: "output_text"` 的模型文本。
    OutputText {
        /// 文本中的引用等标注信息。
        annotations: Vec<Value>,
        /// 模型生成的文本。
        text: String,
        /// 可选的 token 对数概率信息。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        logprobs: Option<Vec<Value>>,
        /// 保留当前未声明的扩展字段。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    /// `type: "refusal"` 的拒绝说明。
    Refusal {
        /// 模型给出的拒绝原因。
        refusal: String,
        /// 保留当前未声明的扩展字段。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
}

#[cfg(test)]
mod tests {
    use super::{Content, InputPart, Message, OutputPart, Phase};
    use serde_json::json;

    #[test]
    fn input_messages_keep_text_multimodal_and_output_history() {
        let source = json!([
            {"role": "developer", "content": "Be concise"},
            {"role": "user", "content": [
                {"type": "input_text", "text": "Describe this"},
                {"type": "input_image", "image_url": "https://example.com/a.png", "detail": "high"},
                {"type": "input_file", "file_id": "file_1"}
            ], "type": "message", "status": "completed", "extension": true},
            {"role": "assistant", "content": "Prior answer", "phase": "final_answer"},
            {"id": "msg_1", "content": [{"type": "output_text", "annotations": [], "text": "Earlier reply"}], "role": "assistant", "status": "completed", "type": "message", "phase": null}
        ]);
        let messages: Vec<Message> = serde_json::from_value(source.clone()).unwrap();

        assert!(matches!(&messages[0], Message::Easy(_)));
        assert!(matches!(&messages[1], Message::Input(input)
            if matches!(&input.content[1], InputPart::InputImage { .. })));
        assert!(matches!(&messages[2], Message::Easy(easy)
            if easy.phase == super::OptionalNullable::Value(Phase::FinalAnswer)));
        assert!(matches!(&messages[3], Message::Output(output)
            if matches!(&output.content[0], OutputPart::OutputText { .. })));
        assert_eq!(serde_json::to_value(messages).unwrap(), source);
    }

    #[test]
    fn easy_message_keeps_string_and_part_array_distinct() {
        let text: Message =
            serde_json::from_value(json!({"role": "user", "content": "hello"})).unwrap();
        assert!(matches!(
            text,
            Message::Easy(super::EasyInputMessage {
                content: Content::Text(_),
                ..
            })
        ));

        let parts: Message = serde_json::from_value(
            json!({"role": "assistant", "content": [{"type": "input_text", "text": "earlier"}]}),
        )
        .unwrap();
        assert!(matches!(
            parts,
            Message::Easy(super::EasyInputMessage {
                content: Content::Parts(_),
                ..
            })
        ));
    }
}
