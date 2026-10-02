//! Responses 请求缓存参数与通用缓存配置的映射。

use crate::{
    adapter::nullable::present,
    ir::cache::CacheSettings,
    protocol::{
        OptionalNullable,
        responses::request::body::{PromptCacheOptions, Request},
    },
};

/// 从 Responses 请求提取显式缓存设置。
pub fn decode_responses_cache(request: &Request) -> CacheSettings {
    let options = match &request.prompt_cache_options {
        OptionalNullable::Value(options) => Some(options),
        _ => None,
    };
    CacheSettings {
        key: present(&request.prompt_cache_key),
        mode: options.and_then(|options| present(&options.mode)),
        ttl: options.and_then(|options| present(&options.ttl)),
        retention: present(&request.prompt_cache_retention),
        reference: None,
    }
}

/// 将 IR 中有值的缓存参数写回请求，保留未映射字段和显式 `null`。
pub fn encode_responses_cache(request: &mut Request, cache: &CacheSettings) {
    if let Some(key) = &cache.key {
        request.prompt_cache_key = OptionalNullable::Value(key.clone());
    }
    if let Some(retention) = &cache.retention {
        request.prompt_cache_retention = OptionalNullable::Value(retention.clone());
    }
    if cache.mode.is_some() || cache.ttl.is_some() {
        if !matches!(request.prompt_cache_options, OptionalNullable::Value(_)) {
            request.prompt_cache_options = OptionalNullable::Value(PromptCacheOptions::default());
        }
        if let OptionalNullable::Value(options) = &mut request.prompt_cache_options {
            if let Some(mode) = &cache.mode {
                options.mode = OptionalNullable::Value(mode.clone());
            }
            if let Some(ttl) = &cache.ttl {
                options.ttl = OptionalNullable::Value(ttl.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{decode_responses_cache, encode_responses_cache};
    use crate::protocol::responses::request::Request;
    use serde_json::json;

    #[test]
    fn cache_projection_keeps_diagnostics_and_nulls() {
        let source = json!({"model":"m","input":"hi","prompt_cache_key":null,"prompt_cache_options":{"mode":"explicit","ttl":"30m","comparison_response_id":"resp_1"}});
        let mut request: Request = serde_json::from_value(source.clone()).unwrap();
        let mut cache = decode_responses_cache(&request);
        assert_eq!(cache.mode.as_deref(), Some("explicit"));
        assert_eq!(cache.key, None);
        encode_responses_cache(&mut request, &cache);
        assert_eq!(serde_json::to_value(&request).unwrap(), source);
        cache.key = Some("next".into());
        encode_responses_cache(&mut request, &cache);
        let encoded = serde_json::to_value(request).unwrap();
        assert_eq!(encoded["prompt_cache_key"], "next");
        assert_eq!(
            encoded["prompt_cache_options"]["comparison_response_id"],
            "resp_1"
        );
    }
}
