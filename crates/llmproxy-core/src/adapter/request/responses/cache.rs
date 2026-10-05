//! Responses 缓存参数的协议字段接入；共用缓存映射算法。
use crate::protocol::responses::request::Request;
use crate::{
    adapter::nullable::{present, set},
    ir::cache::CacheSettings,
};

/// 提取显式缓存配置，消息内容的断点独立保留。
pub fn decode_responses_cache(request: &Request) -> CacheSettings {
    super::super::cache::decode(
        &request.prompt_cache_key,
        &request.prompt_cache_retention,
        &request.prompt_cache_options,
        |options| (present(&options.mode), present(&options.ttl)),
    )
}

/// 仅写回有值的缓存配置，保留缺失、null 和协议扩展。
pub fn encode_responses_cache(request: &mut Request, cache: &CacheSettings) {
    super::super::cache::encode(
        &mut request.prompt_cache_key,
        &mut request.prompt_cache_retention,
        &mut request.prompt_cache_options,
        cache,
        |options, cache| {
            set(&mut options.mode, cache.mode.clone());
            set(&mut options.ttl, cache.ttl.clone());
        },
    )
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
