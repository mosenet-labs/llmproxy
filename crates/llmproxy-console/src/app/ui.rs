pub(crate) mod chat;
pub(crate) mod models;
pub(crate) mod providers;
pub(crate) mod routes;

use topcoat::{
    Result,
    context::Cx,
    icon::icon,
    router::{Slot, error::see_other, href, layout, request::uri, route},
    view::{View, attributes, view},
};
use topcoat_ant_design::head_assets;
use topcoat_ant_design::icons::{APARTMENT_OUTLINED, APPSTORE_OUTLINED, PROJECT_OUTLINED};

const NAV_ITEM: &str = "flex min-h-11 items-center gap-3 whitespace-nowrap rounded-md px-3 py-2.5 text-sm text-secondary hover:bg-surface hover:text-primary aria-[current=page]:bg-primary-soft aria-[current=page]:font-medium aria-[current=page]:text-[#0958d9] max-[900px]:gap-2 max-[900px]:px-2 max-[900px]:text-[13px]";
#[route(GET)]
pub async fn providers_redirect(cx: &Cx) -> Result<topcoat::router::error::SeeOther> {
    Ok(see_other(href!(providers::list).resolve(cx)))
}

#[layout]
pub async fn shell(cx: &Cx, slot: Slot<'_>) -> Result<impl View> {
    let routes_page = uri(cx).path() == "/ui/routes";
    let models_page = uri(cx).path() == "/ui/models";
    let chat_page = uri(cx).path() == "/ui/chat";
    let page_title = if routes_page {
        "路由概览"
    } else if models_page {
        "Models"
    } else if chat_page {
        "Chat"
    } else {
        "Providers"
    };
    let providers_link = href!(providers::list).resolve(cx);
    let models_link = href!(models::models).resolve(cx);
    let chat_link = href!(chat::chat).resolve(cx);
    let routes_link = href!(routes::routes).resolve(cx);
    Ok(view! {
        <!DOCTYPE html>
        <html lang="zh-CN">
            <head>
                <meta charset="utf-8">
                <meta name="viewport" content="width=device-width, initial-scale=1">
                <meta name="color-scheme" content="light">
                <title>(format!("{page_title} · LLMProxy"))</title>
                head_assets()
                <link rel="stylesheet" href="/ui/assets/console.css">
                topcoat::runtime::script()
            </head>
            <body>
                <div class="grid min-h-screen grid-cols-[216px_minmax(0,1fr)] max-[900px]:grid-cols-[176px_minmax(0,1fr)] max-[640px]:block">
                    <aside class="sticky top-0 flex h-screen flex-col border-r border-border bg-white px-3 max-[640px]:static max-[640px]:h-auto max-[640px]:border-r-0 max-[640px]:border-b max-[640px]:px-4 max-[640px]:pb-2">
                        <a class="flex h-16 shrink-0 items-center gap-3 px-3 text-lg font-semibold max-[900px]:gap-2 max-[900px]:px-2 max-[900px]:text-base max-[640px]:h-14 max-[640px]:px-0" href=(providers_link.clone())><span class="grid size-8 place-items-center rounded-lg bg-primary text-xl font-bold text-white" aria-hidden="true">"L"</span><strong>"LLMProxy"</strong></a>
                        <nav class="mt-4 grid gap-1 max-[640px]:mt-0 max-[640px]:grid-cols-2" aria-label="主导航">
                            <a class=(NAV_ITEM) href=(providers_link) aria-current=(if routes_page || models_page || chat_page { None } else { Some("page") })>icon(data: APPSTORE_OUTLINED, attrs: attributes! { class="size-[18px] shrink-0" aria-hidden="true" })"Providers"</a>
                            <a class=(NAV_ITEM) href=(models_link) aria-current=(if models_page { Some("page") } else { None })>icon(data: APPSTORE_OUTLINED, attrs: attributes! { class="size-[18px] shrink-0" aria-hidden="true" })"Models"</a>
                            <a class=(NAV_ITEM) href=(chat_link) aria-current=(if chat_page { Some("page") } else { None })>icon(data: PROJECT_OUTLINED, attrs: attributes! { class="size-[18px] shrink-0" aria-hidden="true" })"Chat"</a>
                            <a class=(NAV_ITEM) href=(routes_link) aria-current=(if routes_page { Some("page") } else { None })>icon(data: APARTMENT_OUTLINED, attrs: attributes! { class="size-[18px] shrink-0" aria-hidden="true" })"路由概览"</a>
                        </nav>
                        <div class="mt-auto border-t border-border px-3 py-5 text-[13px] text-muted max-[640px]:hidden">"本地开发环境"</div>
                    </aside>
                    <div class="flex min-w-0 flex-col">
                        <header class=(if chat_page { "h-12 shrink-0 border-b border-[#e9edf2] bg-white text-xs text-[#788496] max-[640px]:hidden" } else { "h-16 shrink-0 border-b border-border bg-white text-sm text-secondary max-[640px]:hidden" })><div class="mx-auto flex size-full max-w-[1480px] items-center px-7 [&_strong]:font-medium [&_strong]:text-heading"><span>"控制台"<span class="mx-2.5 text-[#b9c3ce]">"/"</span><strong>(page_title)</strong></span></div></header>
                        <main id="main" class=(if chat_page { "mx-auto w-full max-w-[1480px] min-h-0 flex-1 p-4 max-[640px]:p-3" } else { "mx-auto w-full max-w-[1480px] flex-1 p-7 pb-12 max-[640px]:px-4 max-[640px]:pt-6 max-[640px]:pb-8" })>(slot)</main>
                    </div>
                </div>
            </body>
        </html>
    })
}
