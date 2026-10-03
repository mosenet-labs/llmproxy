use super::super::{ConversionWarning, warn};
use crate::{
    adapter::Result,
    ir::request::controls::OutputFormat,
    protocol::{
        OptionalNullable as O, Protocol, Request, chat::request::parameters as c,
        gemini::request::body as g, messages::request::body as m, responses::request::body as r,
    },
};
use serde_json::{Map, Value};
/// Schema 对象直接写入目标字段，不通过完整协议 JSON 进行转换。
pub(super) fn encode(
    body: &mut Request,
    format: Option<&OutputFormat>,
    source: Protocol,
    warnings: &mut Vec<ConversionWarning>,
) -> Result<()> {
    let target = body.protocol();
    let schema = match format {
        Some(OutputFormat::JsonSchema { schema, .. }) => Some(schema.clone()),
        Some(OutputFormat::JsonObject) => Some(Map::from_iter([(
            "type".into(),
            Value::String("object".into()),
        )])),
        _ => None,
    };
    match body {
        Request::Chat(b) => {
            b.response_format = format
                .map(|v| match v {
                    OutputFormat::Text => c::ResponseFormat::Text {
                        extra: Default::default(),
                    },
                    OutputFormat::JsonObject => c::ResponseFormat::JsonObject {
                        extra: Default::default(),
                    },
                    OutputFormat::JsonSchema {
                        name,
                        description,
                        schema,
                        strict,
                    } => c::ResponseFormat::JsonSchema {
                        json_schema: c::JsonSchemaFormat {
                            name: name.clone().unwrap_or_else(|| "response".into()),
                            description: description.clone().into(),
                            schema: O::Value(schema.clone()),
                            strict: (*strict).into(),
                            extra: Default::default(),
                        },
                        extra: Default::default(),
                    },
                })
                .into()
        }
        Request::Responses(b) => {
            let mut config = b.text.as_option().cloned().unwrap_or(r::TextConfig {
                format: O::Missing,
                verbosity: O::Missing,
                extra: Default::default(),
            });
            config.format = format
                .map(|v| match v {
                    OutputFormat::JsonSchema {
                        name,
                        description,
                        schema,
                        strict,
                    } => r::TextFormat {
                        r#type: "json_schema".into(),
                        name: O::Value(name.clone().unwrap_or_else(|| "response".into())),
                        description: description.clone().into(),
                        schema: O::Value(schema.clone()),
                        strict: (*strict).into(),
                        extra: Default::default(),
                    },
                    v => r::TextFormat {
                        r#type: if matches!(v, OutputFormat::Text) {
                            "text"
                        } else {
                            "json_object"
                        }
                        .into(),
                        name: O::Missing,
                        description: O::Missing,
                        schema: O::Missing,
                        strict: O::Missing,
                        extra: Default::default(),
                    },
                })
                .into();
            if format.is_some() || b.text.as_option().is_some() {
                b.text = O::Value(config);
            }
        }
        Request::Messages(b) => {
            if format.is_some() || b.output_config.as_option().is_some() {
                ensure_output_config(&mut b.output_config).format = schema
                    .map(|schema| m::OutputFormat {
                        r#type: "json_schema".into(),
                        schema,
                        extra: Default::default(),
                    })
                    .into();
            }
            if matches!(format, Some(OutputFormat::JsonObject)) {
                warn(
                    warnings,
                    source,
                    target,
                    "output_format",
                    "JSON 对象模式映射为 object Schema",
                );
            }
        }
        Request::Gemini(b) => {
            if format.is_some() || b.generation_config.as_option().is_some() {
                let config = ensure_gemini(&mut b.generation_config);
                config.response_mime_type = format
                    .map(|v| {
                        if matches!(v, OutputFormat::Text) {
                            "text/plain".into()
                        } else {
                            "application/json".into()
                        }
                    })
                    .into();
                config.response_json_schema = match format {
                    Some(OutputFormat::JsonSchema { schema, .. }) => {
                        O::Value(Value::Object(schema.clone()))
                    }
                    _ => O::Missing,
                };
                config.response_schema = O::Missing;
            }
        }
    }
    if matches!(target, Protocol::AnthropicMessages | Protocol::Gemini)
        && let Some(OutputFormat::JsonSchema {
            strict: Some(_), ..
        }) = format
    {
        warn(
            warnings,
            source,
            target,
            "output_format.strict",
            "保留 Schema，目标无独立 strict 开关",
        );
    }
    Ok(())
}

/// 保留同协议未编辑的其他输出配置。
pub(super) fn ensure_output_config(value: &mut O<m::OutputConfig>) -> &mut m::OutputConfig {
    if !matches!(value, O::Value(_)) {
        *value = O::Value(m::OutputConfig {
            effort: O::Missing,
            format: O::Missing,
            extra: Default::default(),
        });
    }
    match value {
        O::Value(value) => value,
        _ => unreachable!(),
    }
}
/// 仅在需要写字段时创建 Gemini generationConfig。
pub(super) fn ensure_gemini(value: &mut O<g::GenerationConfig>) -> &mut g::GenerationConfig {
    if !matches!(value, O::Value(_)) {
        *value = O::Value(g::GenerationConfig::default());
    }
    match value {
        O::Value(value) => value,
        _ => unreachable!(),
    }
}
