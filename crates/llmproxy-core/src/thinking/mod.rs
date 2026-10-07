//! 模型声明思考能力，本轮选择只修改 IR；具体字段仍由 ProtocolCodec 编码。
//! 参考：https://developers.openai.com/api/docs/guides/reasoning
//! 参考：https://platform.claude.com/docs/en/build-with-claude/extended-thinking
//! 参考：https://ai.google.dev/gemini-api/docs/generate-content/thinking
use crate::{
    ir::request::{Request, controls::Reasoning},
    protocol::Protocol,
};
use serde::{Deserialize, Serialize};

/// 网关专用选择字段，上游 HTTP 边界必须移除。
pub const HEADER: &str = "x-llmproxy-thinking";

/// 能力未知与不支持分开保存，旧模型不会被误认为可关闭。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Support {
    #[default]
    Unknown,
    Unsupported,
    Switchable,
    AlwaysOn,
}
impl Support {
    /// 表单与持久化使用相同稳定名称。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Unsupported => "unsupported",
            Self::Switchable => "switchable",
            Self::AlwaysOn => "always_on",
        }
    }
    /// 页面能力状态，不推断目标模型。
    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown => "未配置",
            Self::Unsupported => "不支持思考",
            Self::Switchable => "可开关",
            Self::AlwaysOn => "始终开启",
        }
    }
}

/// 会话偏好与每轮快照共用的三态选择。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Choice {
    #[default]
    Default,
    Enabled,
    Disabled,
}
impl Choice {
    /// 仅接受明确的协议无关意图，未知值不能默认为开启。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "default" => Some(Self::Default),
            "enabled" => Some(Self::Enabled),
            "disabled" => Some(Self::Disabled),
            _ => None,
        }
    }
    /// HTTP 专用头与页面表单共用固定值。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
        }
    }
    /// 每轮显示用户选择，不声称 Provider 实际消耗了思考 token。
    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "思考默认",
            Self::Enabled => "思考开启",
            Self::Disabled => "思考关闭",
        }
    }
}

/// 启用参数复用 IR；能力不是强制覆盖所有请求的策略。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// 管理员确认的实际模型能力。
    pub support: Support,
    /// 开启时应用的模式、强度或预算；第一版不配置摘要。
    pub enabled: Reasoning,
}
impl Config {
    /// 在请求体读取前即可检查能力，错误说明不包含客户端正文。
    pub fn check(&self, choice: Choice) -> Result<(), &'static str> {
        match (choice, self.support) {
            (Choice::Default, _) => Ok(()),
            (_, Support::Unknown) => Err("当前模型尚未配置思考能力，请选择默认"),
            (_, Support::Unsupported) => Err("当前模型不支持思考设置，请选择默认"),
            (Choice::Disabled, Support::AlwaysOn) => Err("当前模型无法关闭思考，请选择默认或开启"),
            _ => Ok(()),
        }
    }

    /// 校验全部已声明的 Provider 协议，避免配置保存后才发现无法编码。
    pub fn validate(&self, protocols: &[Protocol]) -> Result<(), &'static str> {
        if matches!(self.support, Support::Unknown | Support::Unsupported) {
            return if self.enabled == Reasoning::default() {
                Ok(())
            } else {
                Err("未配置或不支持思考时不能保存启用参数")
            };
        }
        if self.enabled.include.is_some() || self.enabled.summary.is_some() {
            return Err("模型启用配置暂不设置思考摘要");
        }
        if self
            .enabled
            .mode
            .as_deref()
            .is_some_and(|m| !matches!(m, "enabled" | "adaptive"))
        {
            return Err("启用模式必须是 enabled 或 adaptive");
        }
        if self
            .enabled
            .effort
            .as_deref()
            .is_some_and(|e| !matches!(e, "minimal" | "low" | "medium" | "high" | "xhigh" | "max"))
        {
            return Err("思考强度无效");
        }
        if self.enabled.budget.is_some_and(|n| n != -1 && n <= 0) {
            return Err("启用预算必须为正整数或 -1");
        }
        for protocol in protocols {
            self.parameters(*protocol)?;
        }
        Ok(())
    }

    /// 按目标协议选择表达方式，不把努力等级换算成 token 预算。
    fn parameters(&self, protocol: Protocol) -> Result<Reasoning, &'static str> {
        let mut r = self.enabled.clone();
        match protocol {
            Protocol::OpenAiChat | Protocol::OpenAiResponses => {
                if r.effort.is_none() || r.budget.is_some() || r.mode.as_deref() == Some("enabled")
                {
                    return Err("Chat／Responses 启用配置需要思考强度，不能使用手动模式或预算");
                }
                r.mode = None;
            }
            Protocol::AnthropicMessages => {
                if r.mode.as_deref() == Some("enabled") || r.budget.is_some_and(|n| n > 0) {
                    if r.budget.is_none_or(|n| n < 1024) || r.mode.as_deref() == Some("adaptive") {
                        return Err(
                            "Messages 手动模式需要至少 1024 的预算，自适应模式不能设置数字预算",
                        );
                    }
                    r.mode = Some("enabled".into());
                } else if r.mode.as_deref() == Some("adaptive")
                    || r.effort.is_some()
                    || r.budget == Some(-1)
                {
                    r.mode = Some("adaptive".into());
                    r.budget = None;
                } else {
                    return Err("Messages 开启时需要自适应模式或手动预算");
                }
            }
            Protocol::Gemini => {
                if r.effort.is_some() && r.budget.is_some() {
                    return Err("Gemini 思考等级与预算不能同时配置");
                }
                if r.effort.is_none() && r.budget.is_none() {
                    if r.mode.as_deref() != Some("adaptive") {
                        return Err("Gemini 开启时需要思考等级或预算");
                    }
                    r.budget = Some(-1);
                }
                r.mode = None;
            }
        }
        Ok(r)
    }

    /// 显式选择清理旧强度；开启时按实际目标请求可见摘要，默认不覆盖原请求。
    pub fn apply(
        &self,
        choice: Choice,
        protocol: Protocol,
        request: &mut Request,
    ) -> Result<(), &'static str> {
        self.check(choice)?;
        if choice == Choice::Default {
            return Ok(());
        }
        let parameters = if choice == Choice::Enabled {
            self.validate(&[protocol])?;
            self.parameters(protocol)?
        } else {
            // 使用目标协议可往返的 IR 表达，避免 Preserve 校验把等价的关闭设置判为丢失。
            match protocol {
                Protocol::OpenAiChat | Protocol::OpenAiResponses => Reasoning {
                    effort: Some("none".into()),
                    ..Default::default()
                },
                Protocol::AnthropicMessages => Reasoning {
                    mode: Some("disabled".into()),
                    ..Default::default()
                },
                Protocol::Gemini => Reasoning {
                    budget: Some(0),
                    ..Default::default()
                },
            }
        };
        if protocol == Protocol::AnthropicMessages
            && parameters.mode.as_deref() == Some("enabled")
            && parameters
                .budget
                .zip(request.generation.max_output_tokens)
                .is_some_and(|(budget, max)| budget as u64 >= max)
        {
            return Err("思考预算必须小于当前请求的输出上限");
        }
        let r = &mut request.generation.reasoning;
        r.mode = parameters.mode;
        r.effort = parameters.effort;
        r.budget = parameters.budget;
        if choice == Choice::Enabled {
            // 摘要意图在选定 Provider 后设置，避免来源 Chat 协议无法表达该开关。
            match protocol {
                Protocol::Gemini => r.include = Some(true),
                Protocol::OpenAiResponses => {
                    // 保留客户端指定的详细程度，缺失时采用模型支持的自动摘要。
                    r.summary.get_or_insert_with(|| "auto".into());
                }
                // Chat 没有标准摘要开关；Messages 随 thinking 返回可见思考。
                Protocol::OpenAiChat | Protocol::AnthropicMessages => {}
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
