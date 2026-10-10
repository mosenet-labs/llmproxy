use super::super::forms::{FIELD, PRIMARY};
use super::*;
use llmproxy_store::auth::UserView;
use topcoat::{runtime::Signal, view::class};
use topcoat_ant_design::{NativeDialogConfig, native_dialog, native_dialog_close_attributes};

#[component]
pub(super) async fn user_editor(
    cx: &Cx,
    user: &UserView,
    refresh: &Signal<f64>,
    csrf: &str,
) -> Result<impl View> {
    let form = FormState::new(cx, Some(refresh));
    let open = form.open.clone();
    let busy = form.busy.clone();
    let dialog_id = format!("user-edit-{}", user.id);
    let form_id = format!("user-edit-form-{}", user.id);
    let name_id = format!("user-name-{}", user.id);
    let role_id = format!("user-role-{}", user.id);
    let submit = forms::submit(cx, &form_id, save_user, &form);
    let close = native_dialog_close_attributes(cx, &dialog_id);
    Ok(view! {
        native_dialog(config: NativeDialogConfig::new(&dialog_id, "编辑用户"), open: Some(&open), busy: &busy,
            language: UiLanguage::ChineseSimplified, attrs: attributes! { class="w-[min(480px,calc(100%_-_32px))]! whitespace-normal [&_.gr-native-dialog-header]:py-4 [&_h2]:m-0 [&_h2]:text-lg" },
            <form id=(form_id.as_str()) method="post" action="/ui/users" class="m-0" (submit)>
                <input type="hidden" name="csrf" value=(csrf)><input type="hidden" name="id" value=(user.id.to_string())><input type="hidden" name="version" value=(user.version.to_string())>
                <div class="grid gap-5 px-6 py-5">
                    forms::feedback(state: &form)
                    <p class="m-0 break-all text-sm text-secondary">(user.email.as_str())</p>
                    <label class=(FIELD) for=(name_id.as_str())>"显示名称"<input id=(name_id.as_str()) name="display_name" value=(user.display_name.as_str()) required=(true) maxlength="80"></label>
                    <label class=(FIELD) for=(role_id.as_str())>"平台角色"<select id=(role_id.as_str()) name="role" class="h-9!"><option value="user" selected=(user.role == UserRole::User)>"普通用户"</option><option value="admin" selected=(user.role == UserRole::Admin)>"平台管理员"</option></select></label>
                    <label class="flex items-center gap-2 text-sm"><input type="checkbox" name="enabled" value="true" checked=(user.enabled)>"启用账户"</label>
                    <p class="m-0 text-xs leading-relaxed text-muted">"角色或启用状态变更会注销全部会话；未验证邮箱的用户不能设为管理员。"</p>
                </div>
                <footer class="flex justify-end gap-3 border-t border-border px-6 py-4"><button type="button" class=(BUTTON) (close)>"取消"</button><button type="submit" class=(class!(BUTTON, PRIMARY)) :disabled=$(busy.get())>"保存"</button></footer>
            </form>
        )
    })
}
