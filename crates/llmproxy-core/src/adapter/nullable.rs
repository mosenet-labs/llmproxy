//! 各协议适配器共用的可选且可为 `null` 的字段操作。

use crate::protocol::OptionalNullable;

/// 仅提取实际存在的字段值；缺失和显式 `null` 均不投影到 IR。
pub(super) fn present<T: Clone>(value: &OptionalNullable<T>) -> Option<T> {
    match value {
        OptionalNullable::Value(value) => Some(value.clone()),
        _ => None,
    }
}

/// 仅在 IR 给出新值时覆盖原字段，保留缺失或显式 `null` 的状态。
pub(super) fn set<T>(target: &mut OptionalNullable<T>, value: Option<T>) {
    if let Some(value) = value {
        *target = OptionalNullable::Value(value);
    }
}
