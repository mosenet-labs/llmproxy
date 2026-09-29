use bytes::Bytes;
use serde_json::Value;

// 同时保留原始字节和解析结果；未修改时不改变空白、字段顺序或数字写法。
pub struct JsonDocument {
    original: Bytes,
    value: Value,
    changed: bool,
}

impl JsonDocument {
    pub fn decode(original: Bytes) -> Result<Self, Bytes> {
        // 无效 JSON 保持原样，沿用现有的上游和客户端行为。
        match serde_json::from_slice(&original) {
            Ok(value) => Ok(Self {
                original,
                value,
                changed: false,
            }),
            Err(_) => Err(original),
        }
    }

    pub fn value(&self) -> &Value {
        &self.value
    }

    #[allow(dead_code)]
    pub fn value_mut(&mut self) -> &mut Value {
        // 只有实际修改才启用 JSON 重新序列化。
        self.changed = true;
        &mut self.value
    }

    pub fn encode(self) -> Bytes {
        if self.changed {
            Bytes::from(serde_json::to_vec(&self.value).expect("JSON value must serialize"))
        } else {
            self.original
        }
    }
}

pub fn complete_sse_prefix(bytes: &[u8], end: bool) -> Option<usize> {
    if end {
        return Some(bytes.len());
    }
    let mut start = 0;
    let mut last_event_end = None;
    // 空行结束一个 SSE 事件；Pingora 分块可能停在行中间，也可能包含多个事件。
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'\n' {
            let line = &bytes[start..index];
            if line.is_empty() || line == b"\r" {
                last_event_end = Some(index + 1);
            }
            start = index + 1;
        }
    }
    last_event_end
}

pub fn sse_json(bytes: &[u8], mut process: impl FnMut(&Value)) {
    let mut data = String::new();
    // 同一事件的多行 data: 需要合并；非 JSON 数据和 [DONE] 标记保持原始字节。
    for line in bytes.split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() {
            process_sse_data(&data, &mut process);
            data.clear();
        } else if let Some(value) = line.strip_prefix(b"data:")
            && let Ok(value) = std::str::from_utf8(value)
        {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(value.trim_start_matches(' '));
        }
    }
    process_sse_data(&data, &mut process);
}

fn process_sse_data(data: &str, process: &mut impl FnMut(&Value)) {
    if data != "[DONE]"
        && !data.is_empty()
        && let Ok(value) = serde_json::from_str::<Value>(data)
    {
        process(&value);
    }
}

#[cfg(test)]
mod tests {
    use super::{JsonDocument, sse_json};
    use bytes::Bytes;
    use serde_json::json;

    #[test]
    fn json_document_keeps_raw_bytes_until_changed() {
        let raw = Bytes::from_static(br#" { "model": "alias" } "#);
        let document = JsonDocument::decode(raw.clone()).unwrap();
        assert_eq!(document.value()["model"], "alias");
        assert_eq!(document.encode(), raw);

        let mut document = JsonDocument::decode(raw).unwrap();
        document.value_mut()["model"] = json!("provider-model");
        assert_eq!(
            document.encode(),
            Bytes::from_static(br#"{"model":"provider-model"}"#)
        );
    }

    #[test]
    fn sse_parser_exposes_json_events() {
        let mut values = Vec::new();
        sse_json(
            b"event: message\r\ndata: {\"x\":1}\r\n\r\ndata: [DONE]\n\n",
            |value| {
                values.push(value.clone());
            },
        );
        assert_eq!(values, vec![json!({"x": 1})]);
    }
}
