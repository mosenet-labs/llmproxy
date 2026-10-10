use super::*;
use topcoat_ant_design::{
    NativeDialogConfig, UiLanguage, native_dialog, native_dialog_close_attributes,
    native_dialog_trigger_attributes,
};

#[topcoat::view::component]
pub(super) async fn password_editor(cx: &Cx, csrf: &str) -> Result<impl View> {
    let form = FormState::new(cx, None);
    let open = form.open.clone();
    let busy = form.busy.clone();
    let submit = forms::submit(cx, "password-form", auth::change_password, &form);
    let trigger = native_dialog_trigger_attributes(cx, "account-password-dialog");
    let close = native_dialog_close_attributes(cx, "account-password-dialog");
    Ok(view! {
        <button type="button" class=(BUTTON) (trigger)>"修改密码"</button>
        native_dialog(config: NativeDialogConfig::new("account-password-dialog", "修改密码"),
            open: Some(&open), busy: &busy, language: UiLanguage::ChineseSimplified,
            attrs: attributes! { class="w-[min(480px,calc(100%_-_32px))]! [&_.gr-native-dialog-header]:py-4 [&_h2]:m-0 [&_h2]:text-lg" },
            <form id="password-form" action="/ui/account" method="post" class="m-0" (submit)>
                <div class="grid max-h-[calc(100dvh_-_210px)] gap-5 overflow-y-auto px-6 py-5">
                    <input type="hidden" name="csrf" value=(csrf)>
                    forms::feedback(state: &form)
                    <label class=(FIELD) for="current-password">"当前密码"<input id="current-password" name="current_password" type="password" required=(true) maxlength="128" autocomplete="current-password"></label>
                    forms::password_fields(prefix: "account", label: "新密码")
                    <p class="m-0 text-xs leading-relaxed text-muted">"修改后，所有设备上的登录会话将失效，需要重新登录。"</p>
                </div>
                <footer class="flex justify-end gap-3 border-t border-border px-6 py-4">
                    <button type="button" class=(BUTTON) (close) :disabled=$(busy.get())>"取消"</button>
                    <button type="submit" class=(class!(BUTTON, PRIMARY)) :disabled=$(busy.get())>"修改密码并重新登录"</button>
                </footer>
            </form>
        )
    })
}
