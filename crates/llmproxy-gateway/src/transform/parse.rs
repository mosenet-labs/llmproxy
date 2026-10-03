//! SSE 事件边界及事件数据读取。
use serde_json::Value;

/// 找到最后一个完整 SSE 事件的结束位置；流结束时返回全部长度。
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

/// 读取完整 SSE 事件中的 JSON `data`，忽略非 JSON 和结束标记。
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

/// 对单个 SSE 事件的数据调用 JSON 处理函数。
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
    use super::sse_json;
    use serde_json::json;

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
