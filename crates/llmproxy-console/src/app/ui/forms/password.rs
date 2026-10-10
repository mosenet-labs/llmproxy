use super::FIELD;
use topcoat::{
    Result,
    context::Cx,
    runtime::{Event, signal},
    view::{View, attributes, component, view},
};

#[component]
pub(crate) async fn password_fields(cx: &Cx, prefix: &str, label: &str) -> Result<impl View> {
    let invalid = signal(cx, || false);
    let fields_id = format!("{prefix}-password-fields");
    let password_id = format!("{prefix}-password");
    let confirmation_id = format!("{prefix}-confirmation");
    let hint_id = format!("{prefix}-password-hint");
    let error_id = format!("{prefix}-confirmation-error");
    let validate = attributes! { cx => @input=$(|_event: Event| {
        let mismatch = raw!(r#"(() => {
            const fields = document.getElementById(${fields_id}.dehydrate());
            const password = fields.querySelector('[name="password"]');
            const confirmation = fields.querySelector('[name="confirmation"]');
            const mismatch = confirmation.value.length > 0 && password.value !== confirmation.value;
            confirmation.setCustomValidity(mismatch ? '两次输入的密码不一致' : '');
            return cx.hydrate(mismatch);
        })()"#, false);
        invalid.set(mismatch);
    }) };
    Ok(view! {
        <div id=(fields_id.as_str()) class="contents" (validate)>
            <div class=(FIELD)>
                <label for=(password_id.as_str())>(label)</label>
                <input id=(password_id.as_str()) name="password" type="password" required=(true) minlength="15" maxlength="128" autocomplete="new-password" aria-describedby=(hint_id.as_str()) placeholder="15–128 个字符">
                <span id=(hint_id.as_str()) class="text-xs font-normal text-muted">"密码须为 15–128 个字符。"</span>
            </div>
            <div class=(FIELD)>
                <label for=(confirmation_id.as_str())>"确认密码"</label>
                <input id=(confirmation_id.as_str()) name="confirmation" type="password" required=(true) minlength="15" maxlength="128" autocomplete="new-password" aria-describedby=(error_id.as_str()) :aria-invalid=$(if invalid.get() { "true" } else { "false" }) :class=$(if invalid.get() { "border-[#ff4d4f]!" } else { "" }) placeholder="再次输入密码">
                <span id=(error_id.as_str()) role="alert" class="text-xs font-normal text-[#cf1322]" :hidden=$(!invalid.get())>"两次输入的密码不一致"</span>
            </div>
        </div>
    })
}
