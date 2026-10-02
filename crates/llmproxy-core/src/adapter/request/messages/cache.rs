//! Messages 顶层缓存控制与通用缓存设置的映射。

use crate::{
    ir::cache::CacheSettings,
    protocol::{
        OptionalNullable,
        messages::request::{body::Request, cache::CacheControl},
    },
};

/// 提取请求级缓存断点；内容块上的独立断点仍随消息保留。
pub fn decode_messages_cache(request: &Request) -> CacheSettings {
    let control = match &request.cache_control {
        OptionalNullable::Value(control) => Some(control),
        _ => None,
    };
    CacheSettings {
        key: None,
        mode: control.map(|control| control.r#type.clone()),
        ttl: control.and_then(|control| match &control.ttl {
            OptionalNullable::Value(ttl) => Some(ttl.clone()),
            _ => None,
        }),
        retention: None,
        reference: None,
    }
}

/// 写回 IR 中有值的请求级缓存设置，保留内容块断点和新增字段。
pub fn encode_messages_cache(request: &mut Request, cache: &CacheSettings) {
    if cache.mode.is_none() && cache.ttl.is_none() {
        return;
    }
    if !matches!(request.cache_control, OptionalNullable::Value(_)) {
        request.cache_control = OptionalNullable::Value(CacheControl {
            r#type: cache.mode.clone().unwrap_or_else(|| "ephemeral".into()),
            ttl: OptionalNullable::Missing,
            extra: Default::default(),
        });
    }
    if let OptionalNullable::Value(control) = &mut request.cache_control {
        if let Some(mode) = &cache.mode {
            control.r#type = mode.clone();
        }
        if let Some(ttl) = &cache.ttl {
            control.ttl = OptionalNullable::Value(ttl.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{decode_messages_cache, encode_messages_cache};
    use crate::protocol::messages::request::Request;
    use serde_json::json;

    #[test]
    fn top_level_cache_keeps_block_breakpoints() {
        let source = json!({"model":"claude","max_tokens":10,"messages":[{"role":"user","content":[{"type":"text","text":"hi","cache_control":{"type":"ephemeral","ttl":"1h"}}]}],"cache_control":{"type":"ephemeral","ttl":"5m","vendor":true}});
        let mut request: Request = serde_json::from_value(source.clone()).unwrap();
        let mut cache = decode_messages_cache(&request);
        assert_eq!(cache.ttl.as_deref(), Some("5m"));
        cache.ttl = Some("1h".into());
        encode_messages_cache(&mut request, &cache);
        let encoded = serde_json::to_value(request).unwrap();
        assert_eq!(encoded["cache_control"]["ttl"], "1h");
        assert_eq!(encoded["cache_control"]["vendor"], true);
        assert_eq!(
            encoded["messages"][0]["content"][0]["cache_control"]["ttl"],
            "1h"
        );
    }
}
