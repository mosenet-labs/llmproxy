//! Chat 与 Responses 共用的缓存参数算法，协议 DTO 和扩展字段由调用方持有。
use crate::{
    adapter::nullable::{present, set},
    ir::cache::CacheSettings,
    protocol::OptionalNullable,
};

/// 从相同语义的字段提取配置；缺失和 null 不生成 IR 值。
pub(super) fn decode<T>(
    key: &OptionalNullable<String>,
    retention: &OptionalNullable<String>,
    options: &OptionalNullable<T>,
    fields: impl FnOnce(&T) -> (Option<String>, Option<String>),
) -> CacheSettings {
    let (mode, ttl) = options.as_option().map(fields).unwrap_or_default();
    CacheSettings {
        key: present(key),
        retention: present(retention),
        mode,
        ttl,
        reference: None,
    }
}

/// 只写入 IR 有值字段；未编辑的 null、诊断和厂商扩展保留在原 DTO 中。
pub(super) fn encode<T: Default>(
    key: &mut OptionalNullable<String>,
    retention: &mut OptionalNullable<String>,
    options: &mut OptionalNullable<T>,
    cache: &CacheSettings,
    fields: impl FnOnce(&mut T, &CacheSettings),
) {
    set(key, cache.key.clone());
    set(retention, cache.retention.clone());
    if cache.mode.is_some() || cache.ttl.is_some() {
        if !matches!(options, OptionalNullable::Value(_)) {
            *options = OptionalNullable::Value(T::default());
        }
        if let OptionalNullable::Value(options) = options {
            fields(options, cache);
        }
    }
}
