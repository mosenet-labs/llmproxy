use std::ops::Range;

pub const MODEL_PREFIX_LIMIT: usize = 64 * 1024;

pub enum Scan {
    Found { alias: String, range: Range<usize> },
    More,
    Missing,
    Invalid,
}

fn space(bytes: &[u8], mut at: usize) -> usize {
    while at < bytes.len() && bytes[at].is_ascii_whitespace() {
        at += 1;
    }
    at
}

fn string_end(bytes: &[u8], start: usize) -> Option<usize> {
    if bytes.get(start) != Some(&b'"') {
        return None;
    }
    let mut at = start + 1;
    while at < bytes.len() {
        match bytes[at] {
            b'\\' => at += 2,
            b'"' => return Some(at + 1),
            _ => at += 1,
        }
    }
    None
}

fn value_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut at = start;
    let mut depth = 0i32;
    let mut quoted = false;
    while at < bytes.len() {
        match bytes[at] {
            b'\\' if quoted => at += 2,
            b'"' => {
                quoted = !quoted;
                at += 1;
            }
            b'{' | b'[' if !quoted => {
                depth += 1;
                at += 1;
            }
            b'}' | b']' if !quoted && depth > 0 => {
                depth -= 1;
                at += 1;
            }
            b',' | b'}' if !quoted && depth == 0 => return Some(at),
            _ => at += 1,
        }
    }
    None
}

pub fn scan_model(bytes: &[u8]) -> Scan {
    let mut at = space(bytes, 0);
    if at == bytes.len() {
        return Scan::More;
    }
    if bytes[at] != b'{' {
        return Scan::Invalid;
    }
    at += 1;
    loop {
        at = space(bytes, at);
        match bytes.get(at) {
            None => return Scan::More,
            Some(b'}') => return Scan::Missing,
            Some(b'"') => {}
            _ => return Scan::Invalid,
        }
        let Some(end) = string_end(bytes, at) else {
            return Scan::More;
        };
        let Ok(key) = serde_json::from_slice::<String>(&bytes[at..end]) else {
            return Scan::Invalid;
        };
        at = space(bytes, end);
        match bytes.get(at) {
            None => return Scan::More,
            Some(b':') => at += 1,
            _ => return Scan::Invalid,
        }
        at = space(bytes, at);
        if at == bytes.len() {
            return Scan::More;
        }
        if key == "model" {
            if bytes[at] != b'"' {
                return Scan::Invalid;
            }
            let Some(end) = string_end(bytes, at) else {
                return Scan::More;
            };
            let Ok(alias) = serde_json::from_slice::<String>(&bytes[at..end]) else {
                return Scan::Invalid;
            };
            return Scan::Found {
                alias,
                range: at..end,
            };
        }
        let Some(end) = value_end(bytes, at) else {
            return Scan::More;
        };
        at = space(bytes, end);
        match bytes.get(at) {
            Some(b',') => at += 1,
            Some(b'}') => return Scan::Missing,
            _ => return Scan::Invalid,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Scan, scan_model};

    #[test]
    fn finds_only_top_level_model_and_its_exact_json_bytes() {
        let body =
            br#" {"metadata":{"model":"wrong"},"input":[{"x":1}],"model":"a\/b" ,"stream":true}"#;
        let Scan::Found { alias, range } = scan_model(body) else {
            panic!("model not found");
        };
        assert_eq!(alias, "a/b");
        assert_eq!(&body[range], br#""a\/b""#);
    }

    #[test]
    fn waits_for_a_split_string_and_rejects_missing_model() {
        assert!(matches!(scan_model(br#"{"model":"par"#), Scan::More));
        assert!(matches!(scan_model(br#"{"input":[]}"#), Scan::Missing));
    }
}
