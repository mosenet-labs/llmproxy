use super::providers::{BUTTON, PAGE_HEADING, PROTOCOLS, protocol_label};
use crate::app::AppState;
use llmproxy_core::protocol::Protocol;
use topcoat::{
    Result,
    context::{Cx, app_context},
    icon::icon,
    router::{href, page},
    view::{View, attributes, view},
};
use topcoat_ant_design::icons::ARROW_RIGHT_OUTLINED;

#[page]
pub async fn routes(cx: &Cx) -> Result<impl View> {
    let all = app_context::<AppState>(cx).store.list_models().await?;
    Ok(view! {
        <section class=(PAGE_HEADING)>
            <div><h1>"路由概览"</h1><p>"查看三个协议入口与已配置的模型数量。"</p></div>
            <a class=(BUTTON) href=(href!(super::models::models))>"管理 Models"</a>
        </section>
        <section class="grid grid-cols-3 gap-5 max-[1180px]:grid-cols-1" id="routes" aria-label="三个协议的模型映射">
            for protocol in PROTOCOLS {
                <a class="group min-w-0 rounded-lg border border-border bg-white p-6 shadow-xs hover:border-[#91caff] max-[640px]:p-5" href=(href!(super::models::models))>
                    <div class="flex items-center gap-3"><span class="grid size-8 shrink-0 place-items-center rounded-md bg-primary-soft text-sm font-semibold text-primary" aria-hidden="true">(match protocol { Protocol::OpenAiChat => "C", Protocol::OpenAiResponses => "R", Protocol::AnthropicMessages => "A" })</span><span class="text-sm font-medium text-heading">(protocol_label(protocol))</span></div>
                    <strong class="mt-6 block truncate text-xl font-semibold leading-7">(all.iter().filter(|model| model.protocols.contains(&protocol)).count())" 个模型"</strong><span class="mt-2 block text-[13px] text-secondary">"按请求中的模型别名匹配"</span>
                    <div class="mt-6 flex items-center justify-between gap-3 border-t border-border pt-4"><code class="truncate font-mono text-[13px] text-secondary">(protocol.upstream_path())</code>icon(data: ARROW_RIGHT_OUTLINED, attrs: attributes! { class="size-4 shrink-0 text-muted group-hover:text-primary" aria-hidden="true" })</div>
                </a>
            }
        </section>
    })
}
