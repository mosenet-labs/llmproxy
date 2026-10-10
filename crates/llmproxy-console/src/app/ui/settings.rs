mod procedures;
pub(crate) use procedures::{save_mail_settings, test_mail_settings};

use super::forms::{self, BUTTON, FIELD, FormState, PRIMARY};
use crate::app::{AppState, auth};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::page,
    runtime::signal,
    view::{View, class, component, view},
};
use topcoat_ant_design::{TagTone, tag};

#[page]
pub async fn settings(cx: &Cx) -> Result<impl View> {
    let identity = auth::session(cx)?;
    let refresh = signal(cx, || 0.0);
    let _revision = refresh.get();
    let saved = app_context::<AppState>(cx).store.mail_settings().await?;
    let enabled = saved.as_ref().is_some_and(|s| s.enabled);
    let password_configured = saved.as_ref().is_some_and(|s| s.password_configured);
    let form = FormState::new(cx, Some(&refresh));
    let busy = form.busy.clone();
    let submit = forms::submit(
        cx,
        "mail-settings-form",
        procedures::save_mail_settings,
        &form,
    );
    let csrf = auth::csrf_token(cx);
    Ok(view! {
        <section class="max-w-[960px] overflow-hidden rounded-lg border border-solid border-border bg-white" aria-labelledby="mail-settings-title">
            <header class="flex items-center justify-between gap-4 border-b border-border px-6 py-4">
                <div><h1 id="mail-settings-title" class="m-0 text-base font-semibold">"邮件服务"</h1><p class="mb-0 mt-1 text-[13px] text-secondary">"用于邮箱注册验证和找回密码，保存后立即生效。"</p></div>
                tag(tone: if enabled { TagTone::Success } else { TagTone::Default }, (if enabled { "已启用" } else if saved.is_some() { "未启用" } else { "未配置" }))
            </header>
            <form id="mail-settings-form" action="/ui/settings" method="post" class="m-0 grid gap-5 p-6" (submit)>
                <input type="hidden" name="csrf" value=(csrf.as_str())>
                if let Some(settings) = &saved { <input type="hidden" name="version" value=(settings.version.to_string())> }
                forms::feedback(state: &form)
                <label class="flex items-center gap-2 text-sm font-medium"><input id="smtp-enabled" type="checkbox" name="enabled" value="true" checked=(saved.as_ref().is_none_or(|s| s.enabled))>"启用邮件服务"</label>
                <div class="grid items-start gap-5 sm:grid-cols-2">
                    <label class=(FIELD) for="smtp-host">"SMTP 服务器"<input id="smtp-host" name="host" value=(saved.as_ref().map(|s| s.host.as_str()).unwrap_or("")) required=(true) maxlength="253" placeholder="smtp.example.com"></label>
                    <label class=(FIELD) for="smtp-from">"发件邮箱"<input id="smtp-from" name="from" type="email" value=(saved.as_ref().map(|s| s.from.as_str()).unwrap_or("")) required=(true) maxlength="254" placeholder="noreply@example.com"></label>
                    <label class=(FIELD) for="smtp-tls">"加密方式"<select id="smtp-tls" name="tls" class="h-9!"><option value="starttls" selected=(saved.as_ref().is_none_or(|s| s.tls == "starttls"))>"STARTTLS"</option><option value="tls" selected=(saved.as_ref().is_some_and(|s| s.tls == "tls"))>"TLS"</option><option value="none" selected=(saved.as_ref().is_some_and(|s| s.tls == "none"))>"无加密（仅本机测试）"</option></select></label>
                    <div class=(FIELD)><label for="smtp-port">"端口"</label><input id="smtp-port" name="port" type="number" min="1" max="65535" value=(saved.as_ref().map(|s| s.port).unwrap_or(587).to_string()) required=(true)><span class="text-xs font-normal text-muted">"STARTTLS 通常为 587，TLS 通常为 465。"</span></div>
                    <div class=(FIELD)><label for="smtp-username">"用户名（可选）"</label><input id="smtp-username" name="username" value=(saved.as_ref().map(|s| s.username.as_str()).unwrap_or("")) maxlength="254" autocomplete="off"><span class="text-xs font-normal leading-relaxed text-muted">"留空使用无认证 SMTP，并清除已保存密码。"</span></div>
                    <div class=(FIELD)><label for="smtp-password">"密码 / 授权码"</label><input id="smtp-password" name="password" type="password" maxlength="1024" autocomplete="new-password" placeholder=(if password_configured { "已配置；留空保留原密码" } else { "填写用户名时需要密码或授权码" })><span class="text-xs font-normal leading-relaxed text-muted">"加密存储，不显示已保存的密码。"</span></div>
                </div>
                <div><button type="submit" class=(class!(BUTTON, PRIMARY)) :disabled=$(busy.get())>$(if busy.get() { "保存中…" } else { "保存配置" })</button></div>
            </form>
            test_form(enabled: enabled, csrf: &csrf, recipient: identity.user.email.as_str())
        </section>
    })
}

#[component]
async fn test_form(cx: &Cx, enabled: bool, csrf: &str, recipient: &str) -> Result<impl View> {
    let form = FormState::new(cx, None);
    let busy = form.busy.clone();
    let submit = forms::submit(cx, "mail-test-form", procedures::test_mail_settings, &form);
    Ok(view! {
        <section class="border-t border-border p-6" aria-labelledby="mail-test-title">
                <h2 id="mail-test-title" class="mb-1 mt-0 text-sm font-semibold">"发送测试邮件"</h2><p class="mb-4 mt-0 text-[13px] text-secondary">"使用已保存的配置发送，请先保存并启用邮件服务。"</p>
                <form id="mail-test-form" action="/ui/settings" method="post" class="m-0 grid gap-4" (submit)>
                    <input type="hidden" name="csrf" value=(csrf)>
                    forms::feedback(state: &form)
                    <div class="flex max-w-[600px] items-end gap-3 max-[640px]:flex-wrap"><label class=(class!(FIELD, "min-w-0 flex-1")) for="smtp-recipient">"收件邮箱"<input id="smtp-recipient" name="recipient" type="email" required=(true) maxlength="254" value=(recipient)></label><button type="submit" class=(class!(BUTTON, "shrink-0")) :disabled=$(if !enabled { true } else { busy.get() })>$(if busy.get() { "发送中…" } else { "发送测试邮件" })</button></div>
                </form>
            </section>
    })
}
