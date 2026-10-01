//! `output[]` 中 `type: "message"` 的项，不包含其他输出项。
//! 参考 API：https://developers.openai.com/api/reference/cli/resources/responses/methods/create

// 此输出消息也允许原样放入下一次请求的 input，因此共用完全相同的 DTO。
pub use crate::protocol::responses::request::message::{
    OutputMessage as Message, OutputPart as ContentPart,
};

#[cfg(test)]
mod tests {
    use super::Message;
    use serde_json::json;

    #[test]
    fn response_message_round_trip() {
        let source = json!({"id":"msg_1","content":[{"type":"output_text","annotations":[],"text":"hello"}],"role":"assistant","status":"completed","type":"message","phase":null});
        let message: Message = serde_json::from_value(source.clone()).unwrap();
        assert_eq!(serde_json::to_value(message).unwrap(), source);
    }
}
