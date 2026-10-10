use super::table::{Pagination, pagination};
use topcoat::{
    Result,
    context::Cx,
    runtime::{Event, signal},
    view::{View, ViewExt, attributes, component, view},
};
use topcoat_ant_design::{DataTableDensity, data_table};

#[component]
pub(super) async fn no_group(cx: &Cx) -> Result<impl View> {
    let __cx = cx;
    Ok(
        view! { <section class="rounded-lg border border-border bg-white px-6 py-12 text-center"><h2 class="m-0 text-base font-semibold">"尚未获授权的资源组"</h2><p class="mb-0 mt-2 text-sm text-secondary">"请联系组织所有者，为你的账号分配可使用的资源组。"</p></section> },
    )
}

pub(super) async fn member_catalog(cx: &Cx, routes: bool) -> Result<impl View> {
    let __cx = cx;
    crate::app::request_connection(cx);
    if crate::app::store(cx).group_id() == 0 {
        return Ok(view! { no_group() }.boxed());
    }
    let scope = if routes { "routes" } else { "models" };
    let store = crate::app::group_store(cx, scope);
    let query = signal(cx, String::new);
    let needle = query.get().trim().to_lowercase();
    let rows: Vec<(String, String)> = if routes {
        store
            .list_routes()
            .await?
            .into_iter()
            .filter(|row| row.enabled)
            .map(|row| (row.name, row.protocol.as_str().to_owned()))
            .collect()
    } else {
        store
            .list_models()
            .await?
            .into_iter()
            .filter(|row| row.provider_enabled)
            .map(|row| {
                (
                    row.alias,
                    row.protocols
                        .iter()
                        .map(|p| p.as_str())
                        .collect::<Vec<_>>()
                        .join(" · "),
                )
            })
            .collect()
    };
    let filtered: Vec<_> = rows
        .into_iter()
        .filter(|(name, _)| name.to_lowercase().contains(&needle))
        .collect();
    let paging = Pagination::new(cx);
    let range = paging.range(filtered.len());
    Ok(view! {
        <section class="overflow-hidden rounded-lg border border-border bg-white">
            <header class="flex flex-wrap items-center justify-between gap-4 px-6 py-4">
                <div><h2 class="m-0 text-base font-semibold">(if routes {"可调用路由"} else {"可调用模型"})</h2><p class="mb-0 mt-1 text-[13px] text-secondary">"当前组织中，所选资源组已授权的调用入口。"</p></div>
                <div class="flex flex-wrap items-center gap-3">super::groups::group_selector(scope: scope)<input type="search" aria-label="搜索名称" placeholder="搜索名称" :value=$(query.get()) @input=$(|e:Event| query.set(e.target.value))></div>
            </header>
            data_table(label: "可调用入口",density: DataTableDensity::Compact,attrs: attributes! { class="[&_td]:px-6! [&_th]:px-6!" },
                <thead><tr><th>"名称"</th><th>"协议"</th></tr></thead>
                <tbody>for row in &filtered[range] { <tr><td>(row.0.as_str())</td><td>(row.1.as_str())</td></tr> }
                    if filtered.is_empty() { <tr><td colspan="2" class="py-8! text-center! text-muted">"暂无可调用入口"</td></tr> }
                </tbody>
            )
            pagination(state: &paging,total: filtered.len(),id: "member-catalog-page-size",label: "调用入口分页")
        </section>
    }.boxed())
}
