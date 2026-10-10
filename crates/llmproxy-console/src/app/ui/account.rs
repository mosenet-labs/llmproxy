mod password;

use super::forms;
use crate::app::auth;
use forms::{BUTTON, FIELD, FormState, PRIMARY};
use topcoat::{
    Result,
    context::Cx,
    icon::icon,
    router::page,
    view::{View, attributes, class, view},
};
use topcoat_ant_design::{
    dropdown_menu, dropdown_menu_content, dropdown_menu_item, dropdown_menu_separator,
    dropdown_menu_trigger, icons::DOWN_OUTLINED,
};

#[topcoat::view::component]
pub(super) async fn header_account(cx: &Cx) -> Result<impl View> {
    let identity = auth::session(cx)?;
    let form = FormState::new(cx, None);
    let busy = form.busy.clone();
    let submit = forms::submit(cx, "header-logout-form", auth::logout, &form);
    let csrf = auth::csrf_token(cx);
    Ok(view! {
        dropdown_menu(attrs: attributes! { id="account-menu" class="shrink-0 text-[13px]" },
            dropdown_menu_trigger(attrs: attributes! { id="account-menu-trigger" class="flex items-center gap-1.5 rounded text-primary hover:text-primary-hover hover:underline focus-visible:outline-2 focus-visible:outline-primary" title=(identity.user.email.as_str()) },
                <span class="max-w-[180px] truncate">(identity.user.display_name.as_str())</span>
                icon(data: DOWN_OUTLINED, size: 12)
            )
            dropdown_menu_content(attrs: attributes! { class="left-auto! right-0 min-w-44 border-solid bg-white! text-secondary! shadow-lg" },
                <a href=(crate::app::scoped_href(cx, "/ui/account")) class="flex items-center rounded-md px-3 py-2 text-sm text-secondary hover:bg-surface hover:text-primary focus-visible:bg-surface">"个人账户"</a>
                dropdown_menu_separator()
                <form id="header-logout-form" method="post" action="/ui/login" class="m-0" (submit)>
                    <input type="hidden" name="csrf" value=(csrf.as_str())>
                    dropdown_menu_item(attrs: attributes! { type="submit" class="border-0 bg-transparent px-3! py-2! text-secondary hover:bg-surface! hover:text-primary" :disabled=$(busy.get()) }, "退出")
                </form>
                forms::feedback(state: &form)
            )
        )
    })
}

#[page]
pub async fn account(cx: &Cx) -> Result<impl View> {
    let identity = auth::session(cx)?;
    let refresh = topcoat::runtime::signal(cx, || 0.0);
    let _revision = refresh.get();
    let user = topcoat::context::app_context::<crate::app::AppState>(cx)
        .store
        .get_user(identity.user.id)
        .await?;
    let profile = FormState::new(cx, Some(&refresh));
    let busy = profile.busy.clone();
    let profile_submit = forms::submit(cx, "profile-form", auth::save_profile, &profile);
    let csrf = auth::csrf_token(cx);
    Ok(view! {
        <div class="max-w-[960px]">
            <section class="min-w-0 rounded-lg border border-solid border-border bg-white" aria-labelledby="account-title">
                <header class="flex items-center justify-between gap-4 border-b border-border px-6 py-4"><h1 id="account-title" class="m-0 text-base font-semibold">"个人账户"</h1>password::password_editor(csrf: &csrf)</header>
                <form id="profile-form" action="/ui/account" method="post" class="m-0 grid gap-5 p-6" (profile_submit)>
                    <input type="hidden" name="csrf" value=(csrf.as_str())>
                    forms::feedback(state: &profile)
                    <dl class="m-0 grid gap-5 text-sm sm:grid-cols-2"><div class="grid gap-2"><dt class="font-medium">"邮箱"</dt><dd class="m-0 break-all text-secondary">(user.email.as_str())</dd></div><div class="grid gap-2"><dt class="font-medium">"平台角色"</dt><dd class="m-0 text-secondary">(if user.role == llmproxy_store::auth::UserRole::Admin { "平台管理员" } else { "普通用户" })</dd></div></dl>
                    <label class=(class!(FIELD, "max-w-[420px]")) for="profile-name">"显示名称"<input id="profile-name" name="display_name" value=(user.display_name.as_str()) required=(true) maxlength="80" autocomplete="nickname"></label>
                    <div><button type="submit" class=(class!(BUTTON, PRIMARY)) :disabled=$(busy.get())>"保存信息"</button></div>
                </form>
            </section>
        </div>
    })
}
