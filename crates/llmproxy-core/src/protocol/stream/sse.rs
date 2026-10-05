//! HTTP 边界共用的 SSE 分帧与类型化事件读写，不进入协议 codec。
//! 参考：https://html.spec.whatwg.org/multipage/server-sent-events.html#parsing-an-event-stream
use super::Event;
use crate::protocol::{Protocol, messages, responses};
use std::io::{self, Write};

/// 完整事件的数据与可选事件名；只保留当前事件，UTF-8 可跨 HTTP 分块。
pub struct Frame {
    /// 本帧的 `event` 字段；缺失时由 JSON 的协议标签确定事件类型。
    pub event: Option<String>,
    /// 所有 `data` 行按 SSE 规则合并的 UTF-8 字节，不包含尾部换行。
    pub data: Vec<u8>,
}

/// 按字节读取 CR、LF 和 CRLF 行边界；容量覆盖未知字段及注释。
pub struct Decoder {
    line: Vec<u8>,
    data: Vec<u8>,
    event: Option<String>,
    has_data: bool,
    skip_lf: bool,
    first: bool,
    bytes: usize,
    limit: usize,
}
impl Decoder {
    /// 单个 SSE 事件的字节上限，与整次响应长度无关。
    pub fn new(limit: usize) -> Self {
        Self {
            line: Vec::new(),
            data: Vec::new(),
            event: None,
            has_data: false,
            skip_lf: false,
            first: true,
            bytes: 0,
            limit,
        }
    }
    /// 每个完整事件立即返回；调用方逐帧处理，不累计一个分块中的所有事件。
    pub fn push(&mut self, byte: u8) -> Result<Option<Frame>, &'static str> {
        if self.skip_lf {
            self.skip_lf = false;
            if byte == b'\n' {
                return Ok(None);
            }
        }
        self.bytes = self
            .bytes
            .checked_add(1)
            .filter(|n| *n <= self.limit)
            .ok_or("SSE 事件超过容量上限")?;
        if byte == b'\r' || byte == b'\n' {
            self.skip_lf = byte == b'\r';
            return self.finish_line();
        }
        self.line.push(byte);
        Ok(None)
    }
    /// 正常 EOF 不能隐式补齐未完成的 data 事件。
    pub fn finish(&self) -> Result<(), &'static str> {
        if self.has_data || !self.line.is_empty() {
            Err("SSE 事件在空行之前断开")
        } else {
            Ok(())
        }
    }
    /// 处理完整行，空行提交当前事件，其余行只改变当前帧状态。
    fn finish_line(&mut self) -> Result<Option<Frame>, &'static str> {
        let bytes = std::mem::take(&mut self.line);
        let bytes = if self.first {
            self.first = false;
            bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes)
        } else {
            &bytes
        };
        let line = std::str::from_utf8(bytes).map_err(|_| "SSE 事件不是 UTF-8")?;
        if line.is_empty() {
            self.bytes = 0;
            let event = self.event.take();
            if !std::mem::take(&mut self.has_data) {
                return Ok(None);
            }
            self.data.pop();
            return Ok(Some(Frame {
                event,
                data: std::mem::take(&mut self.data),
            }));
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "data" => {
                self.has_data = true;
                self.data.extend_from_slice(value.as_bytes());
                self.data.push(b'\n');
            }
            "event" => self.event = (!value.is_empty()).then(|| value.to_owned()),
            _ => {}
        }
        Ok(None)
    }
}

/// JSON 字节直接读取对应协议 struct，不经过整包 Value。
pub fn decode(protocol: Protocol, frame: &Frame) -> Result<Event, &'static str> {
    if frame.data == b"[DONE]" {
        return if protocol == Protocol::OpenAiChat {
            Ok(Event::End(protocol))
        } else {
            Err("协议不支持 DONE 标记")
        };
    }
    let event = match protocol {
        Protocol::OpenAiChat => Event::Chat(Box::new(
            serde_json::from_slice(&frame.data).map_err(|_| "无效 Chat 事件")?,
        )),
        Protocol::Gemini => Event::Gemini(Box::new(
            serde_json::from_slice(&frame.data).map_err(|_| "无效 Gemini 事件")?,
        )),
        Protocol::AnthropicMessages => Event::Messages(Box::new(
            serde_json::from_slice(&frame.data).map_err(|_| "无效 Messages 事件")?,
        )),
        Protocol::OpenAiResponses => Event::Responses(Box::new(
            serde_json::from_slice(&frame.data).map_err(|_| "无效 Responses 事件")?,
        )),
    };
    // Chat/Gemini 的错误外壳可能作为未知顶层字段解析，必须立即终止，不能仅告警后继续。
    let error = match &event {
        Event::Chat(chunk) => chunk.extra.get("error"),
        Event::Gemini(chunk) => chunk.extra.get("error"),
        _ => None,
    };
    if error.is_some_and(|error| !error.is_null()) {
        return Err("Provider 返回错误事件");
    }
    if let Some(kind) = &frame.event {
        let expected = event_name(&event);
        if expected.is_some_and(|expected| expected != kind) {
            return Err("SSE 事件名与 JSON 类型不一致");
        }
    }
    Ok(event)
}

/// 事件名仅由类型化目标事件决定，不复用来源事件名。
fn event_name(event: &Event) -> Option<&str> {
    match event {
        Event::Messages(event) => Some(match &**event {
            messages::response::Event::Known(event) => event.kind(),
            messages::response::Event::Other(event) => &event.r#type,
        }),
        Event::Responses(event) => Some(match &**event {
            responses::response::Event::Known(event) => event.kind(),
            responses::response::Event::Other(event) => &event.r#type,
        }),
        _ => None,
    }
}

/// 序列化期间即限制目标帧容量，避免转义膨胀后才检查。
struct Bounded {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for Bounded {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.bytes.len().saturating_add(bytes.len()) > self.limit {
            return Err(io::Error::other("SSE 输出超过容量上限"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// 目标协议 struct 写入 SSE，Chat 结束标记与其他协议正常 EOF 分开处理。
pub fn encode(event: &Event, limit: usize) -> Result<Vec<u8>, &'static str> {
    if let Event::End(protocol) = event {
        let bytes = if *protocol == Protocol::OpenAiChat {
            b"data: [DONE]\n\n".to_vec()
        } else {
            Vec::new()
        };
        return if bytes.len() <= limit {
            Ok(bytes)
        } else {
            Err("SSE 输出超过容量上限")
        };
    }
    let mut output = Bounded {
        bytes: Vec::new(),
        limit,
    };
    if let Some(name) = event_name(event) {
        output
            .write_all(format!("event: {name}\n").as_bytes())
            .map_err(|_| "SSE 输出超过容量上限")?;
    }
    output
        .write_all(b"data: ")
        .map_err(|_| "SSE 输出超过容量上限")?;
    let encoded = match event {
        Event::Chat(event) => serde_json::to_writer(&mut output, event),
        Event::Responses(event) => serde_json::to_writer(&mut output, event),
        Event::Messages(event) => serde_json::to_writer(&mut output, event),
        Event::Gemini(event) => serde_json::to_writer(&mut output, event),
        Event::End(_) => unreachable!(),
    };
    encoded.map_err(|_| "SSE 输出超过容量上限")?;
    output
        .write_all(b"\n\n")
        .map_err(|_| "SSE 输出超过容量上限")?;
    Ok(output.bytes)
}
