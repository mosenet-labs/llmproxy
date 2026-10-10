pub(crate) mod account;
pub(crate) mod chat;
pub(crate) mod forms;
pub(crate) mod groups;
pub(crate) mod holidays;
pub(crate) mod login;
pub(crate) mod models;
pub(crate) mod organizations;
pub(crate) mod providers;
mod resource_catalog;
pub(crate) mod routes;
pub(crate) mod settings;
pub(crate) mod subscriptions;
mod table;
pub(crate) mod users;

use topcoat::{
    Result,
    context::Cx,
    icon::icon,
    router::{Slot, error::see_other, href, layout, request::uri, route},
    runtime::{PrefetchMode, link_attrs, prefetch_mode},
    view::{View, attributes, view},
};
use topcoat_ant_design::head_assets;
use topcoat_ant_design::icons::{
    APARTMENT_OUTLINED, APPSTORE_OUTLINED, CALENDAR_OUTLINED, PROJECT_OUTLINED, TEAM_OUTLINED,
};

const NAV_ITEM: &str = "flex min-h-11 items-center gap-3 whitespace-nowrap rounded-md px-3 py-2.5 text-sm text-secondary hover:bg-surface hover:text-primary aria-[current=page]:bg-primary-soft aria-[current=page]:font-medium aria-[current=page]:text-[#0958d9] max-[900px]:gap-2 max-[900px]:px-2 max-[900px]:text-[13px]";
#[route(GET)]
pub async fn providers_redirect(cx: &Cx) -> Result<topcoat::router::error::SeeOther> {
    Ok(see_other(crate::app::scoped_href(
        cx,
        href!(chat::chat).resolve(cx),
    )))
}

#[layout]
pub async fn shell(cx: &Cx, slot: Slot<'_>) -> Result<impl View> {
    use topcoat::view::ViewExt;
    if uri(cx).path() == "/ui/login" {
        return Ok(view! { login::document(slot: slot) }.boxed());
    }
    let identity = crate::app::auth::session(cx).ok();
    let is_admin = identity.is_some_and(|s| s.user.role == llmproxy_store::auth::UserRole::Admin);
    let manage = crate::app::store(cx).can_manage_resources();
    let organizations_page = uri(cx).path().starts_with("/ui/organizations");
    let organization_detail_title = if uri(cx).path() == "/ui/organizations/detail" {
        Some(format!(
            "{}详情",
            crate::app::store(cx).current_space().await?.unwrap().name
        ))
    } else {
        None
    };
    let users_page = uri(cx).path() == "/ui/users";
    let account_page = uri(cx).path() == "/ui/account";
    let settings_page = uri(cx).path() == "/ui/settings";
    let keys_page = uri(cx).path() == "/ui/keys";
    let groups_page = uri(cx).path().starts_with("/ui/groups");
    let group_detail = groups_page && uri(cx).path() != "/ui/groups";
    let group_title = if group_detail {
        Some(groups::group::title(cx).await?)
    } else {
        None
    };
    let routes_page = uri(cx).path().starts_with("/ui/routes");
    let models_page = uri(cx).path() == "/ui/models";
    let chat_page = uri(cx).path() == "/ui/chat";
    let holidays_page = uri(cx).path() == "/ui/holidays";
    let subscriptions_page = uri(cx).path() == "/ui/subscriptions";
    let page_title = (if let Some(title) = &organization_detail_title {
        title.as_str()
    } else if organizations_page {
        "组织"
    } else if settings_page {
        "系统设置"
    } else if users_page {
        "用户管理"
    } else if account_page {
        "个人账户"
    } else if let Some(title) = &group_title {
        title.as_str()
    } else if keys_page {
        "虚拟 Keys"
    } else if groups_page {
        "资源组"
    } else if routes_page {
        "Model Routes"
    } else if models_page {
        "Models"
    } else if chat_page {
        "Chat"
    } else if holidays_page {
        "节假日"
    } else if subscriptions_page {
        "订阅节点"
    } else {
        "Providers"
    })
    .to_owned();
    let providers_link = link_attrs(
        cx,
        crate::app::scoped_href(cx, href!(providers::list).resolve(cx)),
        prefetch_mode(cx),
    );
    let home_link = link_attrs(
        cx,
        crate::app::scoped_href(cx, href!(chat::chat).resolve(cx)),
        PrefetchMode::Never,
    );
    let models_link = link_attrs(
        cx,
        crate::app::scoped_href(cx, href!(models::models).resolve(cx)),
        prefetch_mode(cx),
    );
    let chat_link = link_attrs(
        cx,
        crate::app::scoped_href(cx, href!(chat::chat).resolve(cx)),
        PrefetchMode::Never,
    );
    let routes_link = link_attrs(
        cx,
        crate::app::scoped_href(cx, href!(routes::routes).resolve(cx)),
        prefetch_mode(cx),
    );
    let holidays_link = link_attrs(
        cx,
        crate::app::scoped_href(cx, href!(holidays::holidays).resolve(cx)),
        prefetch_mode(cx),
    );
    let subscriptions_link = link_attrs(
        cx,
        crate::app::scoped_href(cx, href!(subscriptions::nodes).resolve(cx)),
        prefetch_mode(cx),
    );
    Ok(view! {
        <!DOCTYPE html>
        <html lang="zh-CN">
            <head>
                <meta charset="utf-8">
                <meta name="viewport" content="width=device-width, initial-scale=1">
                <meta name="color-scheme" content="light">
                <title>(format!("{page_title} · LLMProxy"))</title>
                head_assets()
                <link rel="stylesheet" href=(crate::app::scoped_href(cx, "/ui/assets/console.css"))>
                topcoat::runtime::script()
                <script type="module" src="/ui/assets/chat-resume.js"></script>
            </head>
            <body>
                <div
                    id="console-shell"
                    class="grid min-h-screen grid-cols-[216px_minmax(0,1fr)] max-[900px]:grid-cols-[176px_minmax(0,1fr)] max-[640px]:block"
                >
                    <aside
                        id="console-sidebar"
                        class="sticky top-0 flex h-screen flex-col border-r border-border bg-white px-3 max-[640px]:static max-[640px]:h-auto max-[640px]:border-r-0 max-[640px]:border-b max-[640px]:px-4 max-[640px]:pb-2"
                    >
                        <a
                            class="flex h-16 shrink-0 items-center gap-3 px-3 text-lg font-semibold max-[900px]:gap-2 max-[900px]:px-2 max-[900px]:text-base max-[640px]:h-14 max-[640px]:px-0"
                            (home_link)
                        >
                            <span
                                class="grid size-8 place-items-center rounded-lg bg-primary text-xl font-bold text-white"
                                aria-hidden="true"
                            >
                                "L"
                            </span>
                            <strong>"LLMProxy"</strong>
                        </a>
                        <nav
                            id="console-navigation"
                            class="mt-4 grid gap-1 max-[640px]:mt-0 max-[640px]:grid-cols-2"
                            aria-label="主导航"
                        >
                            <a
                                id="nav-chat"
                                class=(NAV_ITEM)
                                (chat_link)
                                aria-current=(if chat_page { Some("page") } else { None })
                            >
                                icon(
                                    data: PROJECT_OUTLINED,
                                    attrs: attributes! { class="size-[18px] shrink-0" aria-hidden="true" }
                                )
                                "Chat"
                            </a>
                            if manage {
                            <a
                                id="nav-providers"
                                class=(NAV_ITEM)
                                (providers_link)
                                aria-current=(if routes_page
                                    || models_page
                                    || chat_page
                                    || holidays_page
                                    || subscriptions_page
                                    || keys_page
                                    || groups_page
                                    || users_page
                                    || settings_page
                                    || account_page
                                    || organizations_page {
                                    None
                                } else {
                                    Some("page")
                                })
                            >
                                icon(
                                    data: APPSTORE_OUTLINED,
                                    attrs: attributes! { class="size-[18px] shrink-0" aria-hidden="true" }
                                )
                                "Providers"
                            </a>
                            }
                            <a
                                id="nav-models"
                                class=(NAV_ITEM)
                                (models_link)
                                aria-current=(if models_page { Some("page") } else { None })
                            >
                                icon(
                                    data: APPSTORE_OUTLINED,
                                    attrs: attributes! { class="size-[18px] shrink-0" aria-hidden="true" }
                                )
                                "Models"
                            </a>
                            <a
                                id="nav-routes"
                                class=(NAV_ITEM)
                                (routes_link)
                                aria-current=(if routes_page { Some("page") } else { None })
                            >
                                icon(
                                    data: APARTMENT_OUTLINED,
                                    attrs: attributes! { class="size-[18px] shrink-0" aria-hidden="true" }
                                )
                                "Model Routes"
                            </a>
                            if is_admin {
                                <a
                                    id="nav-holidays"
                                    class=(NAV_ITEM)
                                    (holidays_link)
                                    aria-current=(if holidays_page {
                                        Some("page")
                                    } else {
                                        None
                                    })
                                >
                                    icon(
                                        data: CALENDAR_OUTLINED,
                                        attrs: attributes! { class="size-[18px] shrink-0" aria-hidden="true" }
                                    )
                                    "节假日"
                                </a>
                            }
                            if manage {
                            <a
                                id="nav-subscriptions"
                                class=(NAV_ITEM)
                                (subscriptions_link)
                                aria-current=(if subscriptions_page {
                                    Some("page")
                                } else {
                                    None
                                })
                            >
                                icon(
                                    data: APARTMENT_OUTLINED,
                                    attrs: attributes! { class="size-[18px] shrink-0" aria-hidden="true" }
                                )
                                "订阅节点"
                            </a>
                            <a id="nav-groups" class=(NAV_ITEM) href=(crate::app::scoped_href(cx, href!(groups::groups) .resolve(cx))) aria-current=(if groups_page { Some("page") } else { None })>
                                icon(data: TEAM_OUTLINED, size: 16)
                                "资源组"
                            </a>
                            <a
                                id="nav-keys"
                                class=(NAV_ITEM)
                                href=(crate::app::scoped_href(cx, "/ui/keys"))
                                aria-current=(if keys_page { Some("page") } else { None })
                            >
                                icon(data: TEAM_OUTLINED, size: 16)
                                "虚拟 Keys"
                            </a>
                            }
                            <a id="nav-organizations" class=(NAV_ITEM) href=(crate::app::scoped_href(cx, "/ui/organizations")) aria-current=(if organizations_page { Some("page") } else { None })>icon(data: TEAM_OUTLINED, size: 16) "组织"</a>
                            if is_admin {
                                <a id="nav-users" class=(NAV_ITEM) href=(crate::app::scoped_href(cx, "/ui/users")) aria-current=(if users_page { Some("page") } else { None })>
                                    icon(data: TEAM_OUTLINED, size: 16) "用户管理"
                                </a>
                            }
                            <a id="nav-account" class=(NAV_ITEM) href=(crate::app::scoped_href(cx, "/ui/account")) aria-current=(if account_page { Some("page") } else { None })>
                                icon(data: TEAM_OUTLINED, size: 16) "个人账户"
                            </a>
                        </nav>
                        <div class="mt-auto border-t border-border pt-3 max-[640px]:mt-3">
                            if is_admin {
                                <a id="nav-settings" class=(NAV_ITEM) href=(crate::app::scoped_href(cx, "/ui/settings")) aria-current=(if settings_page { Some("page") } else { None })>
                                    <svg class="size-4 shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" aria-hidden="true"><path d="M4 6h7m4 0h5M4 12h2m4 0h10M4 18h10m4 0h2"></path><circle cx="13" cy="6" r="2"></circle><circle cx="8" cy="12" r="2"></circle><circle cx="16" cy="18" r="2"></circle></svg> "系统设置"
                                </a>
                            }
                            <p class="m-0 px-3 py-5 text-[13px] text-muted max-[640px]:hidden">"本地开发环境"</p>
                        </div>
                    </aside>
                    <div id="console-content" class="flex min-w-0 flex-col">
                        <header id="console-header" class="sticky top-0 z-40 min-h-14 shrink-0 border-b border-border bg-white text-sm text-secondary">
                            <div class="flex min-h-14 flex-wrap items-center justify-between gap-x-4 gap-y-2 px-7 py-2 max-[640px]:px-4">
                                <span class="shrink-0">
                                    "控制台"
                                    <span class="mx-2.5 text-[#b9c3ce]">"/"</span>
                                    if group_detail {
                                        <a href=(crate::app::scoped_href(cx, href!(groups::groups) .resolve(cx))) class="text-primary hover:text-primary-hover hover:underline">"资源组"</a>
                                        <span class="mx-2.5 text-[#b9c3ce]">"/"</span>
                                    }
                                    if organization_detail_title.is_some() {
                                        <a href=(crate::app::scoped_href(cx, "/ui/organizations")) class="text-primary hover:underline">"组织"</a>
                                        <span class="mx-2.5 text-[#b9c3ce]">"/"</span>
                                    }
                                    <strong>(page_title.as_str())</strong>
                                </span>
                                <div class="flex flex-wrap items-center gap-5">organizations::space_selector() account::header_account()</div>
                            </div>
                        </header>
                        <main
                            id="main"
                            class=(if chat_page {
                                "w-full min-h-0 flex-1 p-4 max-[640px]:p-3"
                            } else {
                                "w-full flex-1 p-7 pb-12 max-[640px]:px-4 max-[640px]:pt-6 max-[640px]:pb-8"
                            })
                        >
                            (slot)
                        </main>
                    </div>
                </div>
            </body>
        </html>
    }.boxed())
}
