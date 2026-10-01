//! Chat 请求缓存参数与通用缓存配置的映射。

use crate::{
    ir::cache::CacheSettings,
    protocol::chat::{
        OptionalNullable,
        request::{Request, parameters::PromptCacheOptions},
    },
};

/// 提取 Chat 请求显式指定的缓存参数；消息内容块的断点仍随消息 IR 保留。
pub fn decode_chat_cache(request: &Request) -> CacheSettings {
    let options = match &request.prompt_cache_options {
        OptionalNullable::Value(options) => Some(options),
        _ => None,
    };
    CacheSettings {
        key: present(&request.prompt_cache_key),
        mode: options.and_then(|options| present(&options.mode)),
        ttl: options.and_then(|options| present(&options.ttl)),
        retention: present(&request.prompt_cache_retention),
    }
}

/// 将通用缓存配置写回 Chat 请求，保留原有的供应商扩展参数。
pub fn encode_chat_cache(request: &mut Request, cache: &CacheSettings) {
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

/// 将未提供或显式 `null` 的参数都视为没有通用值。
fn present<T: Clone>(value: &OptionalNullable<T>) -> Option<T> {
    match value {
        OptionalNullable::Value(value) => Some(value.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{decode_chat_cache, encode_chat_cache};
    use crate::protocol::chat::request::Request;

    #[test]
    fn cache_options_project_and_keep_other_fields() {
        let source = json!({"model":"m","messages":[],"prompt_cache_key":"session","prompt_cache_options":{"mode":"explicit","ttl":"30m","vendor":true},"prompt_cache_retention":"24h"});
        let mut request: Request = serde_json::from_value(source).unwrap();
        let mut cache = decode_chat_cache(&request);
        assert_eq!(cache.key.as_deref(), Some("session"));
        assert_eq!(cache.mode.as_deref(), Some("explicit"));
        cache.key = Some("next".into());
        encode_chat_cache(&mut request, &cache);
        let encoded = serde_json::to_value(request).unwrap();
        assert_eq!(encoded["prompt_cache_key"], "next");
        assert_eq!(encoded["prompt_cache_options"]["vendor"], true);
    }

    #[test]
    fn cache_round_trip_keeps_null_and_missing_fields() {
        let source = json!({"model":"m","messages":[],"prompt_cache_key":null,"prompt_cache_options":{"mode":null,"vendor":true}});
        let mut request: Request = serde_json::from_value(source.clone()).unwrap();
        let cache = decode_chat_cache(&request);
        assert_eq!(cache.key, None);
        encode_chat_cache(&mut request, &cache);
        assert_eq!(serde_json::to_value(request).unwrap(), source);
    }
}
