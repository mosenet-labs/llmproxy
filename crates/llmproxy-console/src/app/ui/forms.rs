use crate::app::{auth, auth::Outcome};
use auth::send_code;
use topcoat::{
    Result,
    context::Cx,
    runtime::{Event, Signal, signal},
    view::{Attributes, View, attributes, component, view},
};

mod password;
pub(crate) use password::password_fields;

pub(crate) const BUTTON: &str = "inline-flex h-9 items-center justify-center gap-2 rounded-md border border-solid border-control-border bg-white px-4 text-sm font-medium text-heading hover:border-primary hover:text-primary disabled:pointer-events-none disabled:opacity-50";
pub(crate) const PRIMARY: &str = "border-primary! bg-primary! text-white! hover:bg-primary-hover!";
pub(crate) const FIELD: &str = "grid gap-2 text-sm font-medium";

pub(crate) struct FormState {
    pub busy: Signal<bool>,
    pub error: Signal<String>,
    pub success: Signal<String>,
    pub open: Signal<bool>,
    pub refresh: Signal<f64>,
}

impl FormState {
    pub(crate) fn new(cx: &Cx, refresh: Option<&Signal<f64>>) -> Self {
        Self {
            busy: signal(cx, || false),
            error: signal(cx, String::new),
            success: signal(cx, String::new),
            open: signal(cx, || false),
            refresh: refresh.cloned().unwrap_or_else(|| signal(cx, || 0.0)),
        }
    }
}

pub(crate) fn submit<P>(cx: &Cx, form_id: &str, action: P, state: &FormState) -> Attributes
where
    P: topcoat::runtime::TypedProcedure<Args = (String,), Output = Outcome>
        + topcoat::runtime::Surrogated<Surrogate = &'static topcoat::runtime::ProcedureSurrogate<P>>
        + Copy
        + 'static,
{
    let FormState {
        busy,
        error,
        success,
        open,
        refresh,
    } = state;
    let unavailable: Outcome = Err("请求失败或会话已失效，请刷新后重试".into());
    attributes! { cx => @submit=$(async |event: Event| {
        event.prevent_default();
        if busy.get() { return; }
        open.set(true); busy.set(true); error.set("".to_owned()); success.set("".to_owned());
        let _payload = raw!("cx.hydrate(new URLSearchParams(new FormData(document.getElementById(${form_id}))).toString())", String::new());
        let result = raw!("await Promise.resolve(${action}.call(${_payload})).catch(() => ${unavailable})", unavailable.clone());
        busy.set(false);
        if result.is_ok() {
            let message = result.unwrap();
            if raw!("cx.hydrate(${message}.dehydrate().startsWith('/ui/'))", false) {
                let _navigate = raw!("window.location.assign(${message}.dehydrate())", ());
            } else {
                success.set(message); open.set(false); refresh.increment();
                let _clear = raw!("document.getElementById(${form_id})?.querySelectorAll('input[type=password], input[name=code]').forEach(input => input.value = '')", ());
            }
        } else { error.set(result.unwrap_err()); }
    }) }
}

pub(crate) fn code_button(cx: &Cx, form_id: &str, state: &FormState) -> Attributes {
    let FormState {
        busy,
        error,
        success,
        ..
    } = state;
    let unavailable: Outcome = Err("验证码发送失败，请稍后重试".into());
    attributes! { cx => @click=$(async |_event: Event| {
        if busy.get() { return; }
        busy.set(true); error.set("".to_owned()); success.set("".to_owned());
        let _payload = raw!("cx.hydrate(new URLSearchParams([...new FormData(document.getElementById(${form_id}))].filter(([name]) => ['csrf','email','purpose'].includes(name))).toString())", String::new());
        let result = raw!("await Promise.resolve(${send_code}.call(${_payload})).catch(() => ${unavailable})", unavailable.clone());
        busy.set(false);
        if result.is_ok() { success.set(result.unwrap()); } else { error.set(result.unwrap_err()); }
    }) }
}

#[component]
pub(crate) async fn feedback(cx: &Cx, state: &FormState) -> Result<impl View> {
    let __cx = cx;
    let error = &state.error;
    let success = &state.success;
    Ok(view! {
        <p role="alert" class="m-0 rounded-md border border-solid border-[#ffccc7] bg-[#fff2f0] px-3 py-2.5 text-[13px] text-[#cf1322]" :hidden=$(error.get().is_empty())>$(error.get())</p>
        <p role="status" class="m-0 rounded-md border border-solid border-[#b7eb8f] bg-[#f6ffed] px-3 py-2.5 text-[13px] text-[#389e0d]" :hidden=$(success.get().is_empty())>$(success.get())</p>
    })
}
