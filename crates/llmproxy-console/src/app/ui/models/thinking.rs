//! 模型能力表单与类型化输入；聊天页不直接编辑协议字段。
use super::*;
use llmproxy_core::{
    ir::request::controls::Reasoning,
    thinking::{Config, Support},
};

/// 模式和预算为空时保留缺失语义，非思考模型清理隐藏的启用参数。
pub(super) fn parse(
    support: &str,
    mode: &str,
    effort: &str,
    budget: &str,
) -> std::result::Result<Config, StoreError> {
    let support = match support {
        "unknown" => Support::Unknown,
        "unsupported" => Support::Unsupported,
        "switchable" => Support::Switchable,
        "always_on" => Support::AlwaysOn,
        _ => return Err(StoreError::Validation("思考能力状态无效".into())),
    };
    let enabled = if matches!(support, Support::Switchable | Support::AlwaysOn) {
        Reasoning {
            mode: (!mode.is_empty()).then(|| mode.into()),
            effort: (!effort.is_empty()).then(|| effort.into()),
            budget: if budget.trim().is_empty() {
                None
            } else {
                Some(
                    budget
                        .trim()
                        .parse()
                        .map_err(|_| StoreError::Validation("思考预算必须为整数".into()))?,
                )
            },
            ..Default::default()
        }
    } else {
        Reasoning::default()
    };
    Ok(Config { support, enabled })
}

/// 复用模型编辑页的间距、字体和表单控件，仅展开需要填写的启用参数。
#[topcoat::view::component]
pub(super) async fn editor(
    cx: &Cx,
    support: Signal<String>,
    mode: Signal<String>,
    effort: Signal<String>,
    budget: Signal<String>,
) -> Result<impl View> {
    let _ = cx;
    Ok(view! {
        <fieldset class="mt-6 rounded-md border border-border p-4">
            <legend class="px-1 text-sm font-semibold text-heading">
                "思考能力"
            </legend>
            <label class="mb-2 block text-sm" for="model-thinking-support">
                "能力状态"
            </label>
            <select
                id="model-thinking-support"
                :value=$(support.get())
                @change=$(|event: Event| support.set(event.target.value))
            >
                <option value="unknown">"未配置 · 保持当前行为"</option>
                <option value="unsupported">"不支持思考"</option>
                <option value="switchable">"支持开启和关闭"</option>
                <option value="always_on">"始终开启 · 无法关闭"</option>
            </select>
            <div
                class="mt-4 grid grid-cols-3 gap-3 max-[640px]:grid-cols-1"
                :hidden=$(if support.get() == "unknown" {
                    true
                } else {
                    support.get() == "unsupported"
                })
            >
                <div>
                    <label class="mb-2 block text-sm" for="model-thinking-mode">
                        "启用方式"
                    </label>
                    <select
                        id="model-thinking-mode"
                        :value=$(mode.get())
                        @change=$(|event: Event| mode.set(event.target.value))
                    >
                        <option value="">"按强度／预算"</option>
                        <option value="adaptive">"自适应"</option>
                        <option value="enabled">"Messages 手动预算"</option>
                    </select>
                </div>
                <div>
                    <label class="mb-2 block text-sm" for="model-thinking-effort">
                        "启用强度"
                    </label>
                    <select
                        id="model-thinking-effort"
                        :value=$(effort.get())
                        @change=$(|event: Event| effort.set(event.target.value))
                    >
                        <option value="">"不指定"</option>
                        for value in ["minimal", "low", "medium", "high", "xhigh", "max"] {
                            <option value=(value)>(value)</option>
                        }
                    </select>
                </div>
                <div>
                    <label class="mb-2 block text-sm" for="model-thinking-budget">
                        "启用预算"
                    </label>
                    <input
                        id="model-thinking-budget"
                        type="number"
                        min="-1"
                        :value=$(budget.get())
                        @input=$(|event: Event| budget.set(event.target.value))
                        placeholder="可空；-1 为动态预算"
                    >
                </div>
            </div>
            <p class="mt-3 mb-0 text-xs leading-5 text-secondary">
                "按实际 Provider 和模型确认能力。启用参数只在聊天选择开启时应用；Chat／Responses 使用强度，Messages 使用自适应或手动预算，Gemini 使用等级或预算。"
            </p>
        </fieldset>
    })
}
