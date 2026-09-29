use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// 保留可选字段的三种状态：缺失、JSON null 和实际值。
#[derive(Clone, Debug, Default, PartialEq)]
pub enum OptionalNullable<T> {
    /// JSON 对象中没有这个字段。
    #[default]
    Missing,
    /// 字段存在，值为 JSON `null`。
    Null,
    /// 字段存在，值为对应类型的数据。
    Value(T),
}

impl<T> OptionalNullable<T> {
    pub fn is_missing(&self) -> bool {
        matches!(self, Self::Missing)
    }
}

impl<T: Serialize> Serialize for OptionalNullable<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Missing | Self::Null => serializer.serialize_none(),
            Self::Value(value) => value.serialize(serializer),
        }
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for OptionalNullable<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match Option::<T>::deserialize(deserializer)? {
            Some(value) => Self::Value(value),
            None => Self::Null,
        })
    }
}
