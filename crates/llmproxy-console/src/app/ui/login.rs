use super::forms::{self, BUTTON, FIELD, FormState, PRIMARY};
use crate::app::{AppState, auth};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{Slot, page},
    runtime::{Event, Signal, signal},
    view::{Attributes, View, attributes, class, component, view},
};
use topcoat_ant_design::{head_assets, tabs, tabs_content, tabs_list, tabs_trigger};

#[page]
pub async fn login(cx: &Cx) -> Result<impl View> {
    if let Ok(identity) = auth::session(cx) {
        let destination = if identity.user.role == llmproxy_store::auth::UserRole::Admin {
            "/ui/chat"
        } else {
            "/ui/account"
        };
        return Err(topcoat::router::error::see_other(destination).into());
    }
    let state = app_context::<AppState>(cx);
    let initialized = state.store.admin_initialized().await?;
    let selected = signal(cx, || {
        if initialized { "login" } else { "bootstrap" }.to_owned()
    });
    let login_email = signal(cx, String::new);
    let login_state = FormState::new(cx, None);
    let mail_enabled = state.store.mail_enabled().await?;
    Ok(view! {
        <section class="w-full max-w-[440px] rounded-xl border border-solid border-border bg-white p-7 shadow-sm max-[480px]:p-5" aria-labelledby="login-title">
            <h1 id="login-title" class="m-0 text-2xl font-semibold tracking-tight">"欢迎使用 LLMProxy"</h1>
            <p class="mb-6 mt-2 text-sm text-secondary">(if initialized { "登录你的账户，继续管理和使用模型资源。" } else { "首次使用，请先创建管理员账户。" })</p>
            tabs(attrs: attributes! { aria-label="账户认证" },
                tabs_list(attrs: attributes! { role="navigation" aria-label="登录方式" },
                    if !initialized {
                        tabs_trigger(active: $(selected.get() == "bootstrap"), attrs: attributes! { cx => id="auth-tab-bootstrap" href="/ui/login" aria-controls="auth-bootstrap-panel" @click=$(|e: Event| { e.prevent_default(); selected.set("bootstrap".to_owned()); }) }, "初始化管理员")
                    }
                    tabs_trigger(active: $(selected.get() == "login"), attrs: attributes! { cx => id="auth-tab-login" href="/ui/login" aria-controls="auth-login-panel" @click=$(|e: Event| { e.prevent_default(); selected.set("login".to_owned()); }) }, "登录")
                    tabs_trigger(active: $(selected.get() == "register"), attrs: attributes! { cx => id="auth-tab-register" href="/ui/login" aria-controls="auth-register-panel" @click=$(|e: Event| { e.prevent_default(); selected.set("register".to_owned()); }) }, "邮箱注册")
                    tabs_trigger(active: $(selected.get() == "reset"), attrs: attributes! { cx => id="auth-tab-reset" href="/ui/login" aria-controls="auth-reset-panel" @click=$(|e: Event| { e.prevent_default(); selected.set("reset".to_owned()); }) }, "找回密码")
                )
                if !initialized {
                    tabs_content(attrs: attributes! { cx => id="auth-bootstrap-panel" aria-labelledby="auth-tab-bootstrap" :hidden=$(selected.get() != "bootstrap") }, bootstrap_form())
                }
                tabs_content(attrs: attributes! { cx => id="auth-login-panel" aria-labelledby="auth-tab-login" :hidden=$(selected.get() != "login") }, login_form(email: &login_email, form: &login_state))
                tabs_content(attrs: attributes! { cx => id="auth-register-panel" aria-labelledby="auth-tab-register" :hidden=$(selected.get() != "register") }, email_form(registering: true, enabled: mail_enabled, selected: &selected, login_email: &login_email, login_state: &login_state))
                tabs_content(attrs: attributes! { cx => id="auth-reset-panel" aria-labelledby="auth-tab-reset" :hidden=$(selected.get() != "reset") }, email_form(registering: false, enabled: mail_enabled, selected: &selected, login_email: &login_email, login_state: &login_state))
            )
        </section>
    })
}

#[component]
async fn bootstrap_form(cx: &Cx) -> Result<impl View> {
    let form = FormState::new(cx, None);
    let busy = form.busy.clone();
    let submit = forms::submit(cx, "auth-bootstrap-form", auth::bootstrap_admin, &form);
    let csrf = auth::csrf_token(cx);
    Ok(view! {
        <form id="auth-bootstrap-form" action="/ui/login" method="post" class="m-0 grid gap-4" (submit)>
            <input type="hidden" name="csrf" value=(csrf.as_str())>
            forms::feedback(state: &form)
            <label class=(FIELD) for="bootstrap-email">"管理员邮箱"<input id="bootstrap-email" name="email" type="email" required=(true) autocomplete="username" maxlength="254" placeholder="you@example.com"></label>
            forms::password_fields(prefix: "bootstrap", label: "密码")
            <p class="m-0 text-xs leading-relaxed text-muted">"初始化无需邮件验证码，创建后自动登录。完成后此入口关闭，邮箱注册仅创建普通用户。"</p>
            <button type="submit" class=(class!(BUTTON, PRIMARY, "mt-1 w-full")) :disabled=$(busy.get())>$(if busy.get() { "创建中…" } else { "创建管理员并登录" })</button>
        </form>
    })
}

#[component]
async fn login_form(cx: &Cx, email: &Signal<String>, form: &FormState) -> Result<impl View> {
    let busy = form.busy.clone();
    let submit = forms::submit(cx, "auth-login-form", auth::login, form);
    let csrf = auth::csrf_token(cx);
    Ok(view! {
        <form id="auth-login-form" action="/ui/login" method="post" class="m-0 grid gap-4" (submit)>
            <input type="hidden" name="csrf" value=(csrf.as_str())>
            forms::feedback(state: form)
            <label class=(FIELD) for="login-email">"邮箱"<input id="login-email" name="email" type="email" required=(true) autocomplete="username" maxlength="254" :value=$(email.get()) placeholder="you@example.com"></label>
            <label class=(FIELD) for="login-password">"密码"<input id="login-password" name="password" type="password" required=(true) autocomplete="current-password" maxlength="128" placeholder="输入密码"></label>
            <button type="submit" class=(class!(BUTTON, PRIMARY, "mt-1 w-full")) :disabled=$(busy.get())>$(if busy.get() { "登录中…" } else { "登录" })</button>
        </form>
    })
}

#[component]
async fn email_form(
    cx: &Cx,
    registering: bool,
    enabled: bool,
    selected: &Signal<String>,
    login_email: &Signal<String>,
    login_state: &FormState,
) -> Result<impl View> {
    let form = FormState::new(cx, None);
    let busy = form.busy.clone();
    let prefix = if registering { "register" } else { "reset" };
    let id = format!("auth-{prefix}-form");
    let submit = if registering {
        register_submit(cx, &id, &form, selected, login_email, login_state)
    } else {
        forms::submit(cx, &id, auth::reset_password, &form)
    };
    let send = forms::code_button(cx, &id, &form);
    let csrf = auth::csrf_token(cx);
    let email_id = format!("{prefix}-email");
    let code_id = format!("{prefix}-code");
    Ok(view! {
        if !enabled {
            <p role="status" class="m-0 rounded-md bg-surface p-4 text-sm leading-relaxed text-secondary">(if registering { "管理员尚未配置邮件服务，暂时关闭邮箱注册。" } else { "管理员尚未配置邮件服务，暂时无法找回密码。" })</p>
        } else {
            <form id=(id.as_str()) action="/ui/login" method="post" class="m-0 grid gap-4" (submit)>
                <input type="hidden" name="csrf" value=(csrf.as_str())>
                <input type="hidden" name="purpose" value=(if registering { "register" } else { "reset_password" })>
                forms::feedback(state: &form)
                <label class=(FIELD) for=(email_id.as_str())>"邮箱"<input id=(email_id.as_str()) name="email" type="email" required=(true) autocomplete="email" maxlength="254" placeholder="you@example.com"></label>
                <div class=(FIELD)>
                    <label for=(code_id.as_str())>"邮箱验证码"</label>
                    <div class="flex gap-2"><input class="min-w-0 flex-1" id=(code_id.as_str()) name="code" required=(true) inputmode="numeric" pattern="[0-9]{6}" maxlength="6" autocomplete="one-time-code" placeholder="6 位验证码"><button type="button" class=(class!(BUTTON, "shrink-0")) :disabled=$(busy.get()) (send)>"发送验证码"</button></div>
                    <span class="text-xs font-normal text-muted">"验证码 10 分钟有效；发送间隔至少 60 秒。"</span>
                </div>
                forms::password_fields(prefix: prefix, label: if registering { "密码" } else { "新密码" })
                <button type="submit" class=(class!(BUTTON, PRIMARY, "w-full")) :disabled=$(busy.get())>$(if busy.get() { "提交中…" } else if registering { "注册账户" } else { "重置密码" })</button>
            </form>
        }
    })
}

fn register_submit(
    cx: &Cx,
    form_id: &str,
    form: &FormState,
    selected: &Signal<String>,
    login_email: &Signal<String>,
    login_state: &FormState,
) -> Attributes {
    let FormState {
        busy,
        error,
        success,
        ..
    } = form;
    let login_notice = &login_state.success;
    let login_error = &login_state.error;
    let register = auth::register;
    let unavailable: auth::Outcome = Err("请求失败或会话已失效，请刷新后重试".into());
    attributes! { cx => @submit=$(async |event: Event| {
        event.prevent_default();
        if busy.get() { return; }
        busy.set(true); error.set("".to_owned()); success.set("".to_owned());
        let _payload = raw!("cx.hydrate(new URLSearchParams(new FormData(document.getElementById(${form_id}))).toString())", String::new());
        let email = raw!("cx.hydrate(new URLSearchParams(${_payload}.dehydrate()).get('email').trim().toLowerCase())", String::new());
        let result = raw!("await Promise.resolve(${register}.call(${_payload})).catch(() => ${unavailable})", unavailable.clone());
        busy.set(false);
        if result.is_ok() {
            login_email.set(email); login_error.set("".to_owned()); login_notice.set(result.unwrap()); selected.set("login".to_owned());
            let _clear = raw!("document.getElementById(${form_id})?.querySelectorAll('input[type=password], input[name=code]').forEach(input => input.value = '')", ());
            let _clear_login = raw!("document.getElementById('login-password').value = ''", ());
        } else { error.set(result.unwrap_err()); }
    }) }
}

#[component]
pub(super) async fn document(slot: Slot<'_>) -> Result<impl View> {
    Ok(view! {
        <!DOCTYPE html><html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>"登录 · LLMProxy"</title>head_assets()<link rel="stylesheet" href="/ui/assets/console.css">topcoat::runtime::script()</head>
        <body><main class="flex min-h-screen flex-col items-center justify-center gap-7 bg-surface px-4 py-10">
            <a href="/ui/login" class="flex items-center gap-3 text-xl font-semibold"><span class="grid size-9 place-items-center rounded-lg bg-primary text-2xl text-white" aria-hidden="true">"L"</span>"LLMProxy"</a>
            (slot)
            <p class="m-0 text-xs text-muted">"邮箱账户 · 模型资源控制台"</p>
        </main></body></html>
    })
}
