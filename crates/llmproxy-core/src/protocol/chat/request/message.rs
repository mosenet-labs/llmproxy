//! Chat Completions 请求中 `messages` 数组的原始协议结构。
//! 参考 API：https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub use crate::protocol::optional_nullable::OptionalNullable;

/// 原始请求消息。各角色的必需字段由对应变体声明。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Message {
    /// `developer` 角色：开发者提供的指令。
    Developer(ContentMessage<TextPart>),
    /// `system` 角色：系统提供的指令。
    System(ContentMessage<TextPart>),
    /// `user` 角色：用户输入的文本或多模态内容。
    User(ContentMessage<UserPart>),
    /// `assistant` 角色：模型生成的消息或工具调用。
    Assistant {
        /// 此消息引用的先前音频回复。
        #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
        audio: OptionalNullable<AudioReference>,
        /// 消息内容；存在工具调用时可以缺失或为 `null`。
        #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
        content: OptionalNullable<Content<AssistantPart>>,
        /// 旧版函数调用字段；现已由 `tool_calls` 取代。
        #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
        function_call: OptionalNullable<FunctionCall>,
        /// 同一角色中区分参与者的可选名称。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// 模型给出的拒绝说明。
        #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
        refusal: OptionalNullable<String>,
        /// 模型发起的工具调用列表。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_calls: Option<Vec<ToolCall>>,
        /// 保留当前未声明的扩展字段。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    /// `tool` 角色：工具调用的执行结果。
    Tool {
        /// 工具返回的文本或文本内容块。
        content: Content<TextPart>,
        /// 此结果所对应的工具调用 ID。
        tool_call_id: String,
        /// 保留当前未声明的扩展字段。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    /// 旧版 function 消息，保留以读取已有对话历史。
    Function {
        /// 函数返回的内容，可以是字符串或 `null`。
        content: Option<String>,
        /// 被调用函数的名称。
        name: String,
        /// 保留当前未声明的扩展字段。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
}

/// 指令和用户消息共用必需内容、可选名称与扩展字段的原始形状。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContentMessage<P> {
    /// 消息正文，可以是字符串或内容块数组。
    pub content: Content<P>,
    /// 同一角色中区分参与者的可选名称。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 保留当前未声明的扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 消息内容可以是纯文本，也可以是按顺序排列的内容块。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Content<P> {
    /// 纯文本内容。
    Text(String),
    /// 按顺序排列的内容块。
    Parts(Vec<P>),
}

/// system、developer 和 tool 消息支持的文本内容块。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TextPart {
    /// `type: "text"` 的文本内容块。
    Text {
        /// 内容块中的文本。
        text: String,
        /// 在此内容块末尾设置显式提示缓存断点。
        #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
        prompt_cache_breakpoint: OptionalNullable<PromptCacheBreakpoint>,
        /// 保留当前未声明的扩展字段，例如提示缓存断点。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
}

/// user 消息的内容块。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UserPart {
    /// `type: "text"` 的文本输入。
    Text {
        /// 用户输入的文本。
        text: String,
        /// 在此内容块末尾设置显式提示缓存断点。
        #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
        prompt_cache_breakpoint: OptionalNullable<PromptCacheBreakpoint>,
        /// 保留当前未声明的扩展字段，例如提示缓存断点。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    /// `type: "image_url"` 的图片输入。
    ImageUrl {
        /// 图片地址及可选的细节级别。
        image_url: ImageUrl,
        /// 在此内容块末尾设置显式提示缓存断点。
        #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
        prompt_cache_breakpoint: OptionalNullable<PromptCacheBreakpoint>,
        /// 保留当前未声明的扩展字段，例如提示缓存断点。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    /// `type: "input_audio"` 的音频输入。
    InputAudio {
        /// Base64 音频数据及其格式。
        input_audio: InputAudio,
        /// 在此内容块末尾设置显式提示缓存断点。
        #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
        prompt_cache_breakpoint: OptionalNullable<PromptCacheBreakpoint>,
        /// 保留当前未声明的扩展字段，例如提示缓存断点。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    /// `type: "file"` 的文件输入。
    File {
        /// 文件数据、已上传文件 ID 或文件名。
        file: FileInput,
        /// 在此内容块末尾设置显式提示缓存断点。
        #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
        prompt_cache_breakpoint: OptionalNullable<PromptCacheBreakpoint>,
        /// 保留当前未声明的扩展字段，例如提示缓存断点。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
}

/// 图片内容块中的 `image_url` 对象。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImageUrl {
    /// 图片 URL 或 Base64 编码的图片数据。
    pub url: String,
    /// 图片处理细节级别，可选值为 `auto`、`low`、`high`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// 保留当前未声明的扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 音频内容块中的 `input_audio` 对象。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InputAudio {
    /// Base64 编码的音频数据。
    pub data: String,
    /// 音频格式，文档列出的取值为 `wav`、`mp3`。
    pub format: String,
    /// 保留当前未声明的扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 文件内容块中的 `file` 对象。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileInput {
    /// 作为字符串传入的 Base64 编码文件数据。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_data: Option<String>,
    /// 已上传文件的 ID。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_id: Option<String>,
    /// 文件名。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    /// 保留当前未声明的扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// assistant 消息支持文本与拒绝内容块。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssistantPart {
    /// `type: "text"` 的模型文本内容块。
    Text {
        /// 模型生成的文本。
        text: String,
        /// 在此内容块末尾设置显式提示缓存断点。
        #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
        prompt_cache_breakpoint: OptionalNullable<PromptCacheBreakpoint>,
        /// 保留当前未声明的扩展字段，例如提示缓存断点。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    /// `type: "refusal"` 的拒绝内容块。
    Refusal {
        /// 模型给出的拒绝说明。
        refusal: String,
        /// 保留当前未声明的扩展字段。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
}

/// 消息内容块末尾的显式提示缓存断点。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PromptCacheBreakpoint {
    /// 断点模式，标准值为 `explicit`。
    pub mode: String,
    /// 保留供应商扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// assistant 产生的工具调用；函数参数保持原协议中的字符串形式。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolCall {
    /// `type: "function"` 的函数工具调用。
    Function {
        /// 工具调用的唯一 ID，用于匹配后续工具回复。
        id: String,
        /// 函数名称和参数。
        function: FunctionCall,
        /// 保留当前未声明的扩展字段。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    /// `type: "custom"` 的自定义工具调用。
    Custom {
        /// 工具调用的唯一 ID，用于匹配后续工具回复。
        id: String,
        /// 自定义工具名称和输入。
        custom: CustomCall,
        /// 保留当前未声明的扩展字段。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
}

/// 函数工具调用及旧版 `function_call` 共用的字段结构。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FunctionCall {
    /// 函数参数的 JSON 文本；协议以字符串传递，不保证内容一定是有效 JSON。
    pub arguments: String,
    /// 要调用的函数名。
    pub name: String,
    /// 保留当前未声明的扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 自定义工具调用中的 `custom` 对象。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CustomCall {
    /// 传给自定义工具的输入字符串。
    pub input: String,
    /// 自定义工具名。
    pub name: String,
    /// 保留当前未声明的扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// assistant 消息引用的先前音频回复。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioReference {
    /// 先前音频回复的唯一 ID。
    pub id: String,
    /// 保留当前未声明的扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::{Content, Message, OptionalNullable, ToolCall, UserPart};
    use serde_json::json;

    #[test]
    fn request_messages_round_trip() {
        let source = json!([
            {"role": "developer", "content": "Be concise", "name": "policy"},
            {"role": "system", "content": [{"type": "text", "text": "rules", "prompt_cache_breakpoint": {"mode": "explicit"}}]},
            {"role": "user", "content": [
                {"type": "text", "text": "describe"},
                {"type": "image_url", "image_url": {"url": "https://example.com/a.png", "detail": "high"}},
                {"type": "input_audio", "input_audio": {"data": "abc", "format": "wav"}},
                {"type": "file", "file": {"file_id": "file_1"}}
            ], "provider_extension": true},
            {"role": "assistant", "content": null, "tool_calls": [
                {"id": "call_1", "type": "function", "function": {"name": "lookup", "arguments": "{}"}},
                {"id": "call_2", "type": "custom", "custom": {"name": "execute", "input": "x"}}
            ]},
            {"role": "tool", "tool_call_id": "call_1", "content": [{"type": "text", "text": "done"}]},
            {"role": "function", "name": "lookup", "content": null}
        ]);
        let messages: Vec<Message> = serde_json::from_value(source.clone()).unwrap();

        assert!(matches!(&messages[0], Message::Developer(_)));
        assert!(
            matches!(&messages[2], Message::User(super::ContentMessage { content: Content::Parts(parts), .. })
            if matches!(&parts[1], UserPart::ImageUrl { .. }))
        );
        assert!(matches!(&messages[3], Message::Assistant {
            content: OptionalNullable::Null,
            tool_calls: Some(calls), ..
        } if matches!(&calls[1], ToolCall::Custom { .. })));
        assert_eq!(serde_json::to_value(messages).unwrap(), source);
    }

    #[test]
    fn assistant_content_keeps_missing_null_and_value_distinct() {
        for (content, expected) in [
            (None, OptionalNullable::Missing),
            (Some(json!(null)), OptionalNullable::Null),
            (
                Some(json!("hello")),
                OptionalNullable::Value(Content::Text("hello".into())),
            ),
        ] {
            let mut source = json!({"role": "assistant", "tool_calls": [{"id": "c", "type": "function", "function": {"name": "f", "arguments": "{}"}}]});
            if let Some(content) = content {
                source["content"] = content;
            }
            let message: Message = serde_json::from_value(source.clone()).unwrap();
            assert!(matches!(&message, Message::Assistant { content, .. } if content == &expected));
            assert_eq!(serde_json::to_value(message).unwrap(), source);
        }
    }

    #[test]
    fn missing_required_tool_call_id_is_rejected() {
        let source = json!({"role": "tool", "content": "done"});
        assert!(serde_json::from_value::<Message>(source).is_err());
    }
}
