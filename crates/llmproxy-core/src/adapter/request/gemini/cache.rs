//! Gemini 显式缓存引用与通用缓存配置的映射。

use crate::{
    ir::cache::CacheSettings,
    protocol::{OptionalNullable, gemini::request::body::Request},
};

/// 提取 `cachedContent`；Gemini 的缓存创建与 TTL 属于另一条 API。
pub fn decode_gemini_cache(request: &Request) -> CacheSettings {
    CacheSettings {
        reference: match &request.cached_content {
            OptionalNullable::Value(reference) => Some(reference.clone()),
            _ => None,
        },
        ..Default::default()
    }
}

/// 写回明确指定的缓存内容引用，保留未映射请求字段。
pub fn encode_gemini_cache(request: &mut Request, cache: &CacheSettings) {
    if let Some(reference) = &cache.reference {
        request.cached_content = OptionalNullable::Value(reference.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::{decode_gemini_cache, encode_gemini_cache};
    use crate::protocol::gemini::request::Request;
    use serde_json::json;

    #[test]
    fn cached_content_reference_round_trip() {
        let source = json!({"contents":[],"cachedContent":"cachedContents/1","vendor":true});
        let mut request: Request = serde_json::from_value(source.clone()).unwrap();
        let mut cache = decode_gemini_cache(&request);
        assert_eq!(cache.reference.as_deref(), Some("cachedContents/1"));
        encode_gemini_cache(&mut request, &cache);
        assert_eq!(serde_json::to_value(&request).unwrap(), source);
        cache.reference = Some("cachedContents/2".into());
        encode_gemini_cache(&mut request, &cache);
        assert_eq!(
            serde_json::to_value(request).unwrap()["cachedContent"],
            "cachedContents/2"
        );
    }
}
