use llmproxy_core::protocol::{
    Protocol,
    responses::response::{Event, Response, event::KnownEvent},
    stream::{self, sse},
};

/// 不拼接文本增量；非流式结果取上游真实终态的完整 Response。
/// SSE 分帧复用 Core，每帧有界；失败后不可复用。
pub struct Collector {
    decoder: sse::Decoder,
    response_id: Option<String>,
    terminal: Option<Response>,
    failed: bool,
    done_marker: bool,
}

impl Collector {
    pub fn new(frame_limit: usize) -> Self {
        Self {
            decoder: sse::Decoder::new(frame_limit),
            response_id: None,
            terminal: None,
            failed: false,
            done_marker: false,
        }
    }

    pub fn push(&mut self, bytes: &[u8]) -> Result<(), &'static str> {
        if self.failed {
            return Err("响应收集器已失败");
        }
        let result = self.read(bytes);
        if result.is_err() {
            self.failed = true;
            self.terminal = None;
        }
        result
    }

    pub fn finish(self) -> Result<Response, &'static str> {
        if self.failed {
            return Err("响应收集器已失败");
        }
        self.decoder.finish()?;
        self.terminal.ok_or("上游未交付 Responses 终止事件")
    }

    fn read(&mut self, bytes: &[u8]) -> Result<(), &'static str> {
        for byte in bytes {
            if let Some(frame) = self.decoder.push(*byte)? {
                if frame.data == b"[DONE]" && self.terminal.is_some() && !self.done_marker {
                    self.done_marker = true;
                    continue;
                }
                if self.terminal.is_some() {
                    return Err("Responses 终止事件后仍有数据事件");
                }
                let stream::Event::Responses(event) =
                    sse::decode(Protocol::OpenAiResponses, &frame)?
                else {
                    return Err("上游事件协议不是 Responses");
                };
                let event = *event;
                let Event::Known(event) = event else {
                    continue;
                };
                let (lifecycle, terminal_status) = match *event {
                    KnownEvent::Created(value)
                    | KnownEvent::InProgress(value)
                    | KnownEvent::Queued(value) => (value, None),
                    KnownEvent::Completed(value) => (value, Some("completed")),
                    KnownEvent::Failed(value) => (value, Some("failed")),
                    KnownEvent::Incomplete(value) => (value, Some("incomplete")),
                    KnownEvent::Error(_) => return Err("上游返回 Responses 错误事件"),
                    _ => continue,
                };
                if lifecycle.response.object != "response" || lifecycle.response.id.is_empty() {
                    return Err("上游响应标识无效");
                }
                if self
                    .response_id
                    .as_ref()
                    .is_some_and(|id| id != &lifecycle.response.id)
                {
                    return Err("上游混入其他响应的生命周期事件");
                }
                if self.response_id.is_none() {
                    self.response_id = Some(lifecycle.response.id.clone());
                }
                if let Some(status) = terminal_status {
                    if lifecycle.response.status.as_option().map(String::as_str) != Some(status) {
                        return Err("Responses 终止事件与响应状态不一致");
                    }
                    self.terminal = Some(*lifecycle.response);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn frame(kind: &str, response: &Value) -> Vec<u8> {
        format!(
            "event: {kind}\ndata: {}\n\n",
            json!({"type":kind,"response":response,"sequence_number":1})
        )
        .into_bytes()
    }

    fn response(status: &str) -> Value {
        json!({"id":"resp_1","object":"response","created_at":1,"model":"m","status":status,
            "output":[
                {"type":"function_call","id":"fc_1","call_id":"c1","name":"lookup","arguments":"{}","status":"completed"},
                {"type":"reasoning","id":"r1","summary":[{"type":"summary_text","text":"思考"}],"encrypted_content":"opaque"},
                {"type":"future_media","bytes":"YQ=="}
            ],
            "usage":{"input_tokens":10,"output_tokens":2,"total_tokens":12,
                "input_tokens_details":{"cached_tokens":4},"output_tokens_details":{"reasoning_tokens":1}},
            "future_response":{"keep":true}})
    }

    #[test]
    fn fragmented_stream_preserves_real_terminal_snapshot() {
        for (kind, status) in [
            ("response.completed", "completed"),
            ("response.failed", "failed"),
            ("response.incomplete", "incomplete"),
        ] {
            let expected = response(status);
            let mut bytes = frame("response.created", &response("in_progress"));
            bytes.extend(frame(kind, &expected));
            bytes.extend_from_slice(b"data: [DONE]\n\n");
            for chunk_size in [1, 7, bytes.len()] {
                let mut collector = Collector::new(4096);
                for chunk in bytes.chunks(chunk_size) {
                    collector.push(chunk).unwrap();
                }
                assert_eq!(
                    serde_json::to_value(collector.finish().unwrap()).unwrap(),
                    expected
                );
            }
        }
    }

    #[test]
    fn eof_errors_and_capacity_never_produce_success() {
        let mut collector = Collector::new(4096);
        collector
            .push(&frame("response.created", &response("in_progress")))
            .unwrap();
        assert!(collector.finish().is_err());
        let mut collector = Collector::new(4096);
        let complete = frame("response.completed", &response("completed"));
        collector.push(&complete[..complete.len() - 1]).unwrap();
        assert!(collector.finish().is_err());
        for bytes in [
            b"data: {\"type\":\"error\",\"code\":null,\"message\":\"failure\",\"param\":null,\"sequence_number\":0}\n\n".to_vec(),
            frame("response.completed", &response("failed")),
            b"data: [DONE]\n\n".to_vec(),
        ] {
            let mut collector = Collector::new(4096);
            assert!(collector.push(&bytes).is_err());
            assert!(collector.push(&complete).is_err());
            assert!(collector.finish().is_err());
        }
        let mut collector = Collector::new(20);
        assert!(collector.push(&complete).is_err());
        assert!(collector.finish().is_err());
    }

    #[test]
    fn mixed_response_ids_and_post_terminal_events_are_rejected() {
        let mut collector = Collector::new(4096);
        collector
            .push(&frame("response.created", &response("in_progress")))
            .unwrap();
        let mut other = response("completed");
        other["id"] = json!("resp_2");
        assert!(
            collector
                .push(&frame("response.completed", &other))
                .is_err()
        );
        let mut collector = Collector::new(4096);
        collector
            .push(&frame("response.completed", &response("completed")))
            .unwrap();
        assert!(
            collector
                .push(&frame("response.completed", &response("completed")))
                .is_err()
        );
        assert!(collector.finish().is_err());
    }
}
