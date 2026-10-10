use super::*;
use llmproxy_store::{GroupView, organizations::OrganizationMember};

#[component]
pub(super) async fn member_panel(
    cx: &Cx,
    space_id: i64,
    owner: bool,
    refresh: &Signal<f64>,
) -> Result<impl View> {
    let _revision = refresh.get();
    let actor = auth::session(cx)?.user.id;
    let rows = app_context::<crate::app::AppState>(cx)
        .store
        .organization_members(actor, space_id)
        .await?;
    let groups = crate::app::store(cx).list_groups().await?;
    let transfer = crate::app::store(cx)
        .current_space()
        .await?
        .is_some_and(|s| s.role == OrganizationRole::Owner);
    let paging = Pagination::new(cx);
    let range = paging.range(rows.len());
    Ok(view! {
        <section class=(CARD)>
            <header class="px-6 py-4"><h2 class="m-0 text-base font-semibold">"成员"<span class="ml-2 text-sm font-normal text-muted">(rows.len())</span></h2><p class="mb-0 mt-1 text-[13px] text-secondary">"管理员可管理全部资源；成员只能使用明确授权的资源组。"</p></header>
            data_table(label: "组织成员", density: DataTableDensity::Compact, attrs: attributes! { class="min-w-[700px] [&_th]:px-6! [&_td]:px-6!" },
                <thead><tr><th>"邮箱"</th><th>"角色"</th><th>"资源组"</th><th>"账号状态"</th><th class="text-right!">"操作"</th></tr></thead>
                <tbody>
                    #[key(row.id)]
                    for row in &rows[range] {
                        <tr><td>(row.email.as_str())</td><td>(row.role.label())</td><td>(if row.role.can_manage() {"全部资源组".into()} else {format!("{} 个资源组",row.group_ids.len())})</td><td>(if row.enabled {"正常"} else {"已停用"})</td>
                            <td><div class="flex justify-end gap-4">
                                if owner && row.role != OrganizationRole::Owner {
                                    <button type="button" class=(LINK) (native_dialog_trigger_attributes(cx,&format!("member-edit-{}",row.id)))>"编辑权限"</button>
                                    if transfer { <button type="button" class=(LINK) (native_dialog_trigger_attributes(cx,&format!("member-transfer-{}",row.id)))>"转移所有权"</button> }
                                    <button type="button" class="border-0 bg-transparent p-0 text-sm text-[#cf1322]" (native_dialog_trigger_attributes(cx,&format!("member-remove-{}",row.id)))>"移除"</button>
                                }
                                <button type="button" class=(LINK) (native_dialog_trigger_attributes(cx,&format!("member-revoke-keys-{}",row.id)))>"撤销 Keys"</button>
                            </div>
                            if owner && row.role != OrganizationRole::Owner {
                                member_editor(row: row, groups: &groups, space_id: space_id, refresh: refresh)
                                if transfer { confirm_action(row: row, space_id: space_id, action: "transfer", title: "转移组织所有权", description: "转移后，对方成为唯一所有者，你将成为管理员。", refresh: refresh) }
                                confirm_action(row: row, space_id: space_id, action: "remove", title: "移除成员", description: "移除后，该成员无法访问组织。其创建的应用 Key 默认继续有效，可选择同时撤销。", refresh: refresh)
                            }
                            confirm_action(row: row, space_id: space_id, action: "revoke-keys", title: "撤销成员创建的 Keys", description: "将撤销该成员在本组织创建的全部 Key。此操作不可恢复，应用需要更换凭据。", refresh: refresh)
                            </td>
                        </tr>
                    }
                </tbody>
            )
            pagination(state: &paging,total: rows.len(),id: "members-page-size",label: "组织成员分页")
        </section>
    })
}

#[component]
async fn member_editor(
    cx: &Cx,
    row: &OrganizationMember,
    groups: &[GroupView],
    space_id: i64,
    refresh: &Signal<f64>,
) -> Result<impl View> {
    let id = format!("member-edit-{}", row.id);
    let form = format!("{id}-form");
    let state = FormState::new(cx, Some(refresh));
    let busy = state.busy.clone();
    let role = signal(cx, || row.role.as_str().to_owned());
    let csrf = auth::csrf_token(cx);
    Ok(view! {
        native_dialog(config: NativeDialogConfig::new(&id,"编辑成员权限"), open: Some(&state.open), busy: &state.busy, language: UiLanguage::ChineseSimplified,
            <form id=(form.as_str()) class="m-0" (forms::submit(cx,&form,organization_action,&state))>
                <input type="hidden" name="csrf" value=(csrf)><input type="hidden" name="action" value="member"><input type="hidden" name="space_id" value=(space_id.to_string())><input type="hidden" name="id" value=(row.id.to_string())><input type="hidden" name="version" value=(row.version.to_string())>
                <div class="grid gap-5 px-6 py-5">
                    forms::feedback(state: &state)
                    <p class="m-0 text-sm">(row.email.as_str())</p>
                    <label class=(FIELD)>"角色"<select name="role" :value=$(role.get()) @change=$(|event: Event| { role.set(event.target.value); })><option value="member">"成员"</option><option value="admin">"管理员"</option></select></label>
                    <fieldset class="m-0 grid max-h-64 gap-3 overflow-auto rounded-md border border-control-border p-4" :hidden=$(role.get() != "member")>
                        <legend class="px-1 text-sm">"可使用的资源组"</legend>
                        for group in groups {
                            <label class="flex items-center gap-2 text-sm"><input type="checkbox" name="group_id" value=(group.id.to_string()) checked=(row.group_ids.contains(&group.id))>(group.name.as_str())</label>
                        }
                        if groups.is_empty() { <p class="m-0 text-sm text-muted">"当前组织没有资源组。"</p> }
                    </fieldset>
                    <p class="m-0 text-[13px] text-secondary">"成员未勾选任何资源组时，没有模型调用权限。管理员可管理全部资源及 Keys。"</p>
                </div>
                <footer class="flex justify-end gap-3 border-t border-border px-6 py-4"><button class=(BUTTON) type="button" (native_dialog_close_attributes(cx,&id))>"取消"</button><button class=(class!(BUTTON,PRIMARY)) type="submit" :disabled=$(busy.get())>"保存权限"</button></footer>
            </form>
        )
    })
}

#[component]
async fn confirm_action(
    cx: &Cx,
    row: &OrganizationMember,
    space_id: i64,
    action: &str,
    title: &str,
    description: &str,
    refresh: &Signal<f64>,
) -> Result<impl View> {
    let id = format!("member-{action}-{}", row.id);
    let form = format!("{id}-form");
    let state = FormState::new(cx, Some(refresh));
    let busy = state.busy.clone();
    let csrf = auth::csrf_token(cx);
    Ok(view! {
        native_dialog(config: NativeDialogConfig::new(&id,title),open: Some(&state.open),busy: &state.busy,language: UiLanguage::ChineseSimplified,
            <form id=(form.as_str()) class="m-0" (forms::submit(cx,&form,organization_action,&state))>
                <input type="hidden" name="csrf" value=(csrf)><input type="hidden" name="action" value=(action)><input type="hidden" name="space_id" value=(space_id.to_string())><input type="hidden" name="id" value=(row.id.to_string())><input type="hidden" name="version" value=(row.version.to_string())><input type="hidden" name="user_id" value=(row.user_id.to_string())>
                <div class="grid gap-4 px-6 py-5">
                    forms::feedback(state: &state)
                    <p class="m-0 text-sm font-medium">(row.email.as_str())</p><p class="m-0 text-sm leading-relaxed text-secondary">(description)</p>
                    if action == "remove" { <label class="flex items-center gap-2 text-sm"><input type="checkbox" name="revoke_keys" value="true">"同时撤销该成员创建的 Keys"</label> }
                </div>
                <footer class="flex justify-end gap-3 border-t border-border px-6 py-4"><button class=(BUTTON) type="button" (native_dialog_close_attributes(cx,&id))>"取消"</button><button class=(class!(BUTTON,PRIMARY)) type="submit" :disabled=$(busy.get())>"确认"</button></footer>
            </form>
        )
    })
}

#[component]
pub(super) async fn invitation_panel(
    cx: &Cx,
    space_id: i64,
    refresh: &Signal<f64>,
) -> Result<impl View> {
    let _revision = refresh.get();
    let rows = app_context::<crate::app::AppState>(cx)
        .store
        .organization_invitations(auth::session(cx)?.user.id, Some(space_id))
        .await?;
    let state = FormState::new(cx, Some(refresh));
    let busy = state.busy.clone();
    let csrf = auth::csrf_token(cx);
    let paging = Pagination::new(cx);
    let range = paging.range(rows.len());
    Ok(view! {
        <div class="grid gap-5">
            <section class=(CARD)>
                <form id="organization-invite-form" class="m-0 grid gap-4 p-6" (forms::submit(cx,"organization-invite-form",organization_action,&state))>
                    <input type="hidden" name="csrf" value=(csrf)><input type="hidden" name="action" value="invite"><input type="hidden" name="space_id" value=(space_id.to_string())>
                    forms::feedback(state: &state)
                    <div class="flex flex-wrap items-end gap-3"><label class=(class!(FIELD,"min-w-[240px] flex-1"))>"邮箱"<input type="email" name="email" required=(true) placeholder="已注册账号的邮箱"></label>
                        <label class=(FIELD)>"角色"<select name="role"><option value="member">"成员"</option><option value="admin">"管理员"</option></select></label>
                        <button type="submit" class=(class!(BUTTON,PRIMARY)) :disabled=$(busy.get())>"发送邀请"</button>
                    </div>
                    <p class="m-0 text-[13px] text-secondary">"邀请有效期 7 天，受邀人登录后在「组织」页面接受。成员接受邀请后，再为其分配资源组。"</p>
                </form>
            </section>
            <section class=(CARD)>
                <h2 class="m-0 px-6 py-4 text-base font-semibold">"待接受的邀请"</h2>
                data_table(label: "待接受的邀请",density: DataTableDensity::Compact,attrs: attributes! { class="min-w-[540px] [&_th]:px-6! [&_td]:px-6!" },
                    <thead><tr><th>"邮箱"</th><th>"角色"</th><th>"到期时间"</th><th class="text-right!">"操作"</th></tr></thead>
                    <tbody>#[key(row.id)] for row in &rows[range] { <tr><td>(row.email.as_str())</td><td>(row.role.label())</td><td>(chrono::DateTime::from_timestamp(row.expires_at,0).map(|d| d.with_timezone(&chrono::FixedOffset::east_opt(8*3600).unwrap()).format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default())</td><td>invitation_action(invite: row,accept: false,refresh: refresh)</td></tr> }
                        if rows.is_empty() { <tr><td colspan="4" class="py-8! text-center! text-muted">"暂无待接受的邀请"</td></tr> }
                    </tbody>
                )
                pagination(state: &paging,total: rows.len(),id: "invitations-page-size",label: "组织邀请分页")
            </section>
        </div>
    })
}
