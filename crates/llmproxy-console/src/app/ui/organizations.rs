use super::{
    forms::{self, BUTTON, FIELD, FormState, PRIMARY},
    table::{Pagination, pagination},
};
use crate::app::auth;
use llmproxy_store::organizations::{OrganizationInvitation, OrganizationRole, SpaceKind};
use topcoat::{
    Result,
    context::{Cx, app_context, request_context},
    router::{href, page},
    runtime::{Event, Signal, signal},
    view::{View, attributes, class, component, view},
};
use topcoat_ant_design::{
    DataTableDensity, NativeDialogConfig, TagTone, UiLanguage, data_table, native_dialog,
    native_dialog_close_attributes, native_dialog_trigger_attributes, tabs, tabs_content,
    tabs_list, tabs_trigger, tag,
};

mod members;
mod procedures;
pub(crate) use procedures::{organization_action, select};

const CARD: &str = "min-w-0 overflow-hidden rounded-lg border border-solid border-border bg-white";
const LINK: &str = "border-0 bg-transparent p-0 text-sm text-primary hover:text-primary-hover";

#[component]
pub(crate) async fn space_selector(cx: &Cx) -> Result<impl View> {
    let resources = request_context::<auth::ResourceStore>(cx);
    let space = crate::app::store(cx).current_space().await?.unwrap();
    // Read afresh when a procedure updates organization names or memberships.
    let spaces = app_context::<crate::app::AppState>(cx)
        .store
        .list_spaces(auth::session(cx)?.user.id)
        .await?;
    let csrf = auth::csrf_token(cx);
    Ok(view! {
        <form method="post" action="/ui/organizations/select" class="m-0 flex items-center gap-2" aria-label="切换空间">
            <input type="hidden" name="csrf" value=(csrf)>
            <label for="header-space" class="sr-only">"当前空间"</label>
            <select id="header-space" name="space_id" class="h-8! max-w-[220px] text-[13px]!" aria-label="当前空间">
                #[key(item.id)]
                for item in &spaces {
                    <option value=(item.id.to_string()) selected=(item.id == resources.space.id)>(if item.kind == SpaceKind::Personal { "个人空间".to_owned() } else if item.enabled { item.name.clone() } else { format!("{}（已停用）", item.name) })</option>
                }
            </select>
            <button type="submit" class="border-0 bg-transparent px-1 text-[13px] text-primary hover:underline">"切换"</button>
            <span class="hidden text-[12px] text-muted min-[1100px]:inline">(if space.kind == SpaceKind::Personal { "个人" } else { space.role.label() })</span>
        </form>
    })
}

#[page]
pub async fn organizations(cx: &Cx) -> Result<impl View> {
    crate::app::request_connection(cx);
    let state = FormState::new(cx, None);
    let _revision = state.refresh.get();
    let actor = auth::session(cx)?.user.id;
    let store = &app_context::<crate::app::AppState>(cx).store;
    let rows = store
        .list_spaces(actor)
        .await?
        .into_iter()
        .filter(|s| s.kind == SpaceKind::Organization)
        .collect::<Vec<_>>();
    let invites = store.organization_invitations(actor, None).await?;
    let paging = Pagination::new(cx);
    let range = paging.range(rows.len());
    Ok(view! {
        <div class="grid gap-5">
            <header class="flex flex-wrap items-center justify-between gap-3">
                <p class="m-0 text-sm text-secondary">"个人空间独立使用，组织空间与成员共享。使用顶部切换器选择当前空间。"</p>
                <button class=(class!(BUTTON, PRIMARY)) type="button" (native_dialog_trigger_attributes(cx,"organization-create"))>"创建组织"</button>
            </header>
            forms::feedback(state: &state)
            if !invites.is_empty() {
                <section class=(CARD)>
                    <h2 class="m-0 border-b border-border px-6 py-4 text-base font-semibold">"待接受的邀请"</h2>
                    <div class="grid divide-y divide-border">
                        #[key(invite.id)]
                        for invite in &invites {
                            <div class="flex flex-wrap items-center justify-between gap-3 px-6 py-4">
                                <div><strong class="text-sm">(invite.space_name.as_str())</strong><span class="ml-3 text-[13px] text-muted">(invite.role.label())</span></div>
                                invitation_action(invite: invite, accept: true, refresh: &state.refresh)
                            </div>
                        }
                    </div>
                </section>
            }
            <section class=(CARD)>
                <header class="px-6 py-4"><h2 class="m-0 text-base font-semibold">"组织"<span class="ml-2 text-sm font-normal text-muted">(rows.len())</span></h2></header>
                data_table(label: "组织列表", density: DataTableDensity::Compact, attrs: attributes! { class="min-w-[540px] [&_th]:px-6! [&_td]:px-6!" },
                    <thead><tr><th>"名称"</th><th>"我的角色"</th><th>"状态"</th><th class="text-right!">"操作"</th></tr></thead>
                    <tbody>
                        #[key(row.id)]
                    for row in &rows[range] {
                            <tr><td class="font-medium">(row.name.as_str())</td><td>(row.role.label())</td><td>tag(tone: if row.enabled {TagTone::Success} else {TagTone::Default}, (if row.enabled {"已启用"} else {"已停用"}))</td>
                                <td class="text-right!"><a class=(LINK) href=(format!("/ui/organizations/detail?space={}",row.id))>"详情"</a></td>
                            </tr>
                        }
                        if rows.is_empty() { <tr><td colspan="4" class="py-8! text-center! text-muted">"尚未加入组织，可以创建组织或等待邀请。"</td></tr> }
                    </tbody>
                )
                pagination(state: &paging, total: rows.len(), id: "organizations-page-size", label: "组织列表分页")
            </section>
        </div>
        create_dialog(refresh: &state.refresh)
    })
}

#[component]
async fn create_dialog(cx: &Cx, refresh: &Signal<f64>) -> Result<impl View> {
    let state = FormState::new(cx, Some(refresh));
    let busy = state.busy.clone();
    let csrf = auth::csrf_token(cx);
    Ok(view! {
        native_dialog(config: NativeDialogConfig::new("organization-create","创建组织"), open: Some(&state.open), busy: &state.busy, language: UiLanguage::ChineseSimplified,
            <form id="organization-create-form" class="m-0" (forms::submit(cx,"organization-create-form",organization_action,&state))>
                <input type="hidden" name="csrf" value=(csrf)><input type="hidden" name="action" value="create">
                <div class="grid gap-4 px-6 py-5">
                    forms::feedback(state: &state)
                    <label class=(FIELD)>"组织名称"<input name="name" required=(true) maxlength="80" placeholder="例如：研发团队"></label>
                    <p class="m-0 text-[13px] text-secondary">"创建后你将成为所有者。新组织拥有独立的 Provider、模型、路由、资源组和 Keys。"</p>
                </div>
                <footer class="flex justify-end gap-3 border-t border-border px-6 py-4">
                    <button class=(BUTTON) type="button" (native_dialog_close_attributes(cx,"organization-create"))>"取消"</button>
                    <button class=(class!(BUTTON,PRIMARY)) type="submit" :disabled=$(busy.get())>"创建组织"</button>
                </footer>
            </form>
        )
    })
}

#[component]
async fn invitation_action(
    cx: &Cx,
    invite: &OrganizationInvitation,
    accept: bool,
    refresh: &Signal<f64>,
) -> Result<impl View> {
    let state = FormState::new(cx, Some(refresh));
    let busy = state.busy.clone();
    let form_id = format!("invitation-{}", invite.id);
    let csrf = auth::csrf_token(cx);
    Ok(view! {
        <form id=(form_id.as_str()) class="m-0 grid gap-2" (forms::submit(cx,&form_id,organization_action,&state))>
            <input type="hidden" name="csrf" value=(csrf)><input type="hidden" name="action" value=(if accept {"accept"} else {"revoke-invite"})>
            <input type="hidden" name="space_id" value=(invite.space_id.to_string())><input type="hidden" name="id" value=(invite.id.to_string())><input type="hidden" name="version" value=(invite.version.to_string())>
            forms::feedback(state: &state)
            <button class=(BUTTON) type="submit" :disabled=$(busy.get())>(if accept {"接受邀请"} else {"撤销邀请"})</button>
        </form>
    })
}

#[page("./detail")]
pub async fn detail(cx: &Cx) -> Result<impl View> {
    crate::app::request_connection(cx);
    let state = FormState::new(cx, None);
    let busy = state.busy.clone();
    let _revision = state.refresh.get();
    let space = crate::app::store(cx).current_space().await?.unwrap();
    if space.kind != SpaceKind::Organization {
        return Err(topcoat::router::error::not_found().into());
    }
    let selected = signal(cx, || "basic".to_owned());
    let identity = auth::session(cx)?;
    let owner = space.role == OrganizationRole::Owner
        || identity.user.role == llmproxy_store::auth::UserRole::Admin;
    let csrf = auth::csrf_token(cx);
    let base = format!("/ui/organizations/detail?space={}", space.id);
    Ok(view! {
        <header class="mb-5 flex flex-wrap items-center justify-between gap-3">
            <div class="flex items-center gap-3"><strong class="text-base">(space.name.as_str())</strong>tag((space.role.label())) if !space.enabled { tag(tone: TagTone::Warning,"已停用") }</div>
            <a class=(BUTTON) href=(crate::app::scoped_href(cx,href!(organizations).resolve(cx)))>"返回组织列表"</a>
        </header>
        tabs(
            tabs_list(attrs: attributes! { aria-label="组织设置" },
                tabs_trigger(active: $(selected.get() == "basic"), attrs: attributes! { cx => href=(base.as_str()) @click=$(|e:Event| { e.prevent_default(); selected.set("basic".to_owned()); }) },"基本信息")
                if space.role.can_manage() {
                    tabs_trigger(active: $(selected.get() == "members"), attrs: attributes! { cx => href=(base.as_str()) @click=$(|e:Event| { e.prevent_default(); selected.set("members".to_owned()); }) },"成员")
                }
                if owner {
                    tabs_trigger(active: $(selected.get() == "invitations"), attrs: attributes! { cx => href=(base.as_str()) @click=$(|e:Event| { e.prevent_default(); selected.set("invitations".to_owned()); }) },"邀请")
                }
            )
            tabs_content(attrs: attributes! { cx => :hidden=$(selected.get() != "basic") },
                <section class=(CARD)>
                    <form id="organization-basic-form" class="m-0 grid max-w-2xl gap-5 p-6" (forms::submit(cx,"organization-basic-form",organization_action,&state))>
                        <input type="hidden" name="csrf" value=(csrf)><input type="hidden" name="action" value="update"><input type="hidden" name="space_id" value=(space.id.to_string())><input type="hidden" name="version" value=(space.version.to_string())>
                        forms::feedback(state: &state)
                        <label class=(FIELD)>"组织名称"<input name="name" value=(space.name.as_str()) required=(true) maxlength="80" readonly=(!owner)></label>
                        if owner {
                            <label class="flex items-center gap-2 text-sm"><input type="checkbox" name="enabled" value="true" checked=(space.enabled)>"启用组织"</label>
                            <p class="m-0 text-[13px] text-secondary">"停用会立即阻止组织内全部 Key 的调用，重新启用后恢复。个人空间不受影响。"</p>
                            <div><button class=(class!(BUTTON,PRIMARY)) type="submit" :disabled=$(busy.get())>"保存修改"</button></div>
                        } else { <p class="m-0 text-sm text-secondary">"组织基本信息由所有者管理。"</p> }
                    </form>
                </section>
            )
            if space.role.can_manage() {
                tabs_content(attrs: attributes! { cx => :hidden=$(selected.get() != "members") }, members::member_panel(space_id: space.id, owner: owner, refresh: &state.refresh))
            }
            if owner {
                tabs_content(attrs: attributes! { cx => :hidden=$(selected.get() != "invitations") }, members::invitation_panel(space_id: space.id, refresh: &state.refresh))
            }
        )
    })
}
