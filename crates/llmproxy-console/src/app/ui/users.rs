use super::{
    forms::{self, BUTTON, FormState},
    table::{Pagination, pagination},
};
use crate::app::{AppState, auth, check_csrf};
use llmproxy_store::auth::UserRole;
use serde::Deserialize;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{content::Form, page},
    runtime::{Event, procedure, signal},
    view::{View, attributes, component, view},
};
use topcoat_ant_design::{
    DataTableDensity, TagTone, UiLanguage, data_table, native_dialog_trigger_attributes, tag,
};

mod editor;

#[page]
pub async fn users(cx: &Cx) -> Result<impl View> {
    crate::app::request_connection(cx);
    let refresh = signal(cx, || 0.0);
    let _revision = refresh.get();
    Ok(view! { workspace(refresh: &refresh) })
}

#[component]
async fn workspace(cx: &Cx, refresh: &topcoat::runtime::Signal<f64>) -> Result<impl View> {
    let state = app_context::<AppState>(cx);
    let form = FormState::new(cx, Some(refresh));
    let busy = form.busy.clone();
    let query = signal(cx, String::new);
    let role = signal(cx, String::new);
    let status = signal(cx, String::new);
    let paging = Pagination::new(cx);
    let page = paging.page.clone();
    let needle = query.get().trim().to_lowercase();
    let selected_role = role.get();
    let selected_status = status.get();
    let rows: Vec<_> = state
        .store
        .list_users()
        .await?
        .into_iter()
        .filter(|u| {
            (needle.is_empty()
                || u.email.contains(&needle)
                || u.display_name.to_lowercase().contains(&needle))
                && (selected_role.is_empty() || u.role.as_str() == selected_role)
                && match selected_status.as_str() {
                    "enabled" => u.enabled && u.email_verified,
                    "disabled" => !u.enabled,
                    "unverified" => u.enabled && !u.email_verified,
                    _ => true,
                }
        })
        .collect();
    let range = paging.range(rows.len());
    let csrf = auth::csrf_token(cx);
    let mail_enabled = state.store.mail_enabled().await?;
    Ok(view! {
        <section class="grid min-w-0 gap-4" aria-labelledby="users-title">
            forms::feedback(state: &form)
            <section class="min-w-0 overflow-hidden rounded-lg border border-solid border-border bg-white">
                <header class="flex flex-wrap items-center justify-between gap-4 px-6 py-4 max-[640px]:px-4">
                    <div><h1 id="users-title" class="m-0 text-base font-semibold">"用户列表"<span class="ml-2 text-sm font-normal text-muted">(rows.len())</span></h1><p class="mb-0 mt-1 text-[13px] text-secondary">"管理平台账户与登录权限；组织角色将在组织内单独分配。"</p></div>
                    <div class="flex flex-wrap items-center gap-2">
                        <input type="search" aria-label="搜索用户" placeholder="搜索邮箱或显示名称" class="w-[240px] max-[640px]:w-full" :value=$(query.get()) @input=$(|e: Event| { query.set(e.target.value); page.set(1); })>
                        <select aria-label="平台角色" class="h-9! w-[140px]" :value=$(role.get()) @change=$(|e: Event| { role.set(e.target.value); page.set(1); })><option value="">"全部角色"</option><option value="admin">"平台管理员"</option><option value="user">"普通用户"</option></select>
                        <select aria-label="用户状态" class="h-9! w-[120px]" :value=$(status.get()) @change=$(|e: Event| { status.set(e.target.value); page.set(1); })><option value="">"全部状态"</option><option value="enabled">"正常"</option><option value="unverified">"未验证"</option><option value="disabled">"已禁用"</option></select>
                    </div>
                </header>
                data_table(label: "用户列表", density: DataTableDensity::Compact,
                    attrs: attributes! { class="min-w-[920px] [&_th]:px-6! [&_td]:px-6!" },
                    <thead><tr><th>"用户"</th><th>"平台角色"</th><th>"状态"</th><th>"注册时间"</th><th>"最近登录"</th><th class="text-right!">"操作"</th></tr></thead>
                    <tbody>
                        #[key(user.id)]
                        for user in &rows[range.clone()] {
                            let cannot_reset = !mail_enabled || !user.email_verified || !user.enabled;
                            <tr data-user-id=(user.id.to_string())>
                                <td><strong class="block max-w-[260px] truncate font-medium" title=(user.display_name.as_str())>(user.display_name.as_str())</strong><span class="mt-0.5 block max-w-[260px] truncate text-xs text-muted" title=(user.email.as_str())>(user.email.as_str())</span></td>
                                <td>(if user.role == UserRole::Admin { "平台管理员" } else { "普通用户" })</td>
                                <td>tag(tone: if !user.enabled { TagTone::Default } else if user.email_verified { TagTone::Success } else { TagTone::Warning }, (if !user.enabled { "已禁用" } else if user.email_verified { "正常" } else { "未验证" }))</td>
                                <td class="whitespace-nowrap text-[13px] text-secondary">(format_time(Some(user.created_at)))</td>
                                <td class="whitespace-nowrap text-[13px] text-secondary">(format_time(user.last_login_at))</td>
                                <td><div class="flex justify-end gap-3 whitespace-nowrap">
                                    <button type="button" class="border-0 bg-transparent p-0 text-sm text-primary" (native_dialog_trigger_attributes(cx, &format!("user-edit-{}", user.id)))>"编辑"</button>
                                    <form id=(format!("user-revoke-{}", user.id)) method="post" action="/ui/users" class="m-0" (forms::submit(cx, &format!("user-revoke-{}", user.id), user_action, &form))>
                                        <input type="hidden" name="csrf" value=(csrf.as_str())><input type="hidden" name="id" value=(user.id.to_string())><input type="hidden" name="action" value="revoke_sessions">
                                        <button type="submit" class="border-0 bg-transparent p-0 text-sm text-primary" :disabled=$(busy.get())>"注销会话"</button>
                                    </form>
                                    <form id=(format!("user-reset-{}", user.id)) method="post" action="/ui/users" class="m-0" (forms::submit(cx, &format!("user-reset-{}", user.id), user_action, &form))>
                                        <input type="hidden" name="csrf" value=(csrf.as_str())><input type="hidden" name="id" value=(user.id.to_string())><input type="hidden" name="action" value="reset_email">
                                        <button type="submit" class="border-0 bg-transparent p-0 text-sm text-primary disabled:text-muted" :disabled=$(if cannot_reset { true } else { busy.get() })>"重置邮件"</button>
                                    </form>
                                </div></td>
                            </tr>
                        }
                        if rows.is_empty() { <tr><td colspan="6" class="py-10! text-center text-muted">"没有匹配的用户"</td></tr> }
                    </tbody>
                )
                pagination(state: &paging, total: rows.len(), id: "users-page-size", label: "用户列表分页")
            </section>
            <p class="m-0 text-[13px] text-muted">"禁用或变更角色会注销该用户的全部会话；不能禁用或降级最后一个有效管理员。"</p>
            if !mail_enabled { <p role="status" class="m-0 text-[13px] text-muted">"邮件服务未启用，邮箱注册、密码找回及发送重置邮件暂不可用。"<a href=(crate::app::scoped_href(cx, "/ui/settings")) class="ml-2 text-primary hover:underline">"配置邮件服务"</a></p> }
        </section>
        #[key(user.id)]
        for user in &rows[range.clone()] { editor::user_editor(user: user, refresh: refresh, csrf: csrf.as_str()) }
    })
}

fn format_time(time: Option<i64>) -> String {
    time.and_then(|t| chrono::DateTime::from_timestamp(t, 0))
        .map(|t| {
            (t + chrono::Duration::hours(8))
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| "—".into())
}

#[derive(Deserialize)]
pub struct UserForm {
    csrf: String,
    id: i64,
    #[serde(default)]
    version: u64,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    role: String,
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    action: String,
}

#[procedure("/ui/_topcoat/runtime/procedures/save-user")]
pub async fn save_user(cx: &Cx, payload: String) -> Result<auth::Outcome> {
    let Form(input) = Form::<UserForm>::from_bytes(payload.as_bytes())?;
    check_csrf(cx, &input.csrf)?;
    let actor = auth::session(cx)?;
    let role = match input.role.as_str() {
        "admin" => UserRole::Admin,
        "user" => UserRole::User,
        _ => return Ok(Err("请选择有效的平台角色".into())),
    };
    Ok(app_context::<AppState>(cx)
        .store
        .update_user(
            actor.user.id,
            input.id,
            input.version,
            &input.display_name,
            role,
            input.enabled,
        )
        .await
        .map(|u| {
            if u.id == actor.user.id && (u.role != actor.user.role || !u.enabled) {
                "/ui/login".into()
            } else {
                "用户信息已保存".into()
            }
        })
        .map_err(auth::message))
}

#[procedure("/ui/_topcoat/runtime/procedures/user-action")]
pub async fn user_action(cx: &Cx, payload: String) -> Result<auth::Outcome> {
    let Form(input) = Form::<UserForm>::from_bytes(payload.as_bytes())?;
    check_csrf(cx, &input.csrf)?;
    let actor = auth::session(cx)?;
    let state = app_context::<AppState>(cx);
    match input.action.as_str() {
        "revoke_sessions" => Ok(state
            .store
            .revoke_user_sessions(actor.user.id, input.id)
            .await
            .map(|_| {
                if actor.user.id == input.id {
                    "/ui/login".into()
                } else {
                    "用户全部登录会话已注销".into()
                }
            })
            .map_err(auth::message)),
        "reset_email" => {
            let user = match state.store.get_user(input.id).await {
                Ok(user) => user,
                Err(error) => return Ok(Err(auth::message(error))),
            };
            Ok(auth::deliver_code(
                cx,
                &user.email,
                llmproxy_store::auth::EmailPurpose::ResetPassword,
            )
            .await)
        }
        _ => Ok(Err("不支持的用户操作".into())),
    }
}
