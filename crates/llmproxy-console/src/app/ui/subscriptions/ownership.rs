use crate::app::{auth, check_csrf, ui::forms};
use forms::{BUTTON, FIELD, FormState, PRIMARY};
use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    router::content::Form,
    runtime::procedure,
    view::{View, attributes, class, component, view},
};
use topcoat_ant_design::{
    NativeDialogConfig, UiLanguage, native_dialog, native_dialog_close_attributes,
    native_dialog_trigger_attributes,
};

#[derive(Deserialize)]
struct ClaimForm {
    csrf: String,
    node_id: String,
    node_key: String,
}

#[procedure("/ui/_topcoat/runtime/procedures/claim-subscription")]
pub(crate) async fn claim_subscription(cx: &Cx, payload: String) -> Result<auth::Outcome> {
    let Form(input) = Form::<ClaimForm>::from_bytes(payload.as_bytes())?;
    check_csrf(cx, &input.csrf)?;
    Ok(crate::app::store(cx)
        .claim_subscription(&input.node_id, &input.node_key)
        .await
        .map(|_| "节点已关联，可在列表中启用并导入模型".into())
        .map_err(|error| error.to_string()))
}

#[component]
pub(super) async fn claim_editor(cx: &Cx, state: &FormState, csrf: &str) -> Result<impl View> {
    let open = state.open.clone();
    let busy = state.busy.clone();
    let submit = forms::submit(cx, "claim-node-form", claim_subscription, state);
    let trigger = native_dialog_trigger_attributes(cx, "claim-node-dialog");
    let close = native_dialog_close_attributes(cx, "claim-node-dialog");
    Ok(view! {
        <button type="button" class=(class!(BUTTON, PRIMARY)) (trigger)>
            "关联节点"
        </button>
        native_dialog(
            config: NativeDialogConfig::new("claim-node-dialog", "关联订阅节点"),
            open: Some(&open),
            busy: &busy,
            language: UiLanguage::ChineseSimplified,
            attrs: attributes! {
                class="w-[min(520px,calc(100%_-_32px))]! [&_.gr-native-dialog-header]:py-4 [&_h2]:m-0 [&_h2]:text-lg"
            },
            <form
                id="claim-node-form"
                action="/ui/subscriptions"
                method="post"
                class="m-0"
                (submit)
            >
                <div class="grid gap-5 px-6 py-5">
                    <input type="hidden" name="csrf" value=(csrf)>
                    forms::feedback(state: state)
                    <p class="m-0 text-[13px] leading-relaxed text-secondary">
                        "先启动订阅代理并连接本系统，再从代理状态目录的 node.json 读取节点 ID 和密钥。仅可关联尚未启用、尚未归属账户的节点。"
                    </p>
                    <label class=(FIELD) for="claim-node-id">
                        "节点 ID"
                        <input
                            id="claim-node-id"
                            name="node_id"
                            required=(true)
                            minlength="64"
                            maxlength="64"
                            autocomplete="off"
                        >
                    </label>
                    <label class=(FIELD) for="claim-node-key">
                        "节点密钥"
                        <input
                            id="claim-node-key"
                            name="node_key"
                            type="password"
                            required=(true)
                            minlength="64"
                            maxlength="64"
                            autocomplete="off"
                        >
                    </label>
                </div>
                <footer class="flex justify-end gap-3 border-t border-border px-6 py-4">
                    <button
                        type="button"
                        class=(BUTTON)
                        (close)
                        :disabled=$(busy.get())
                    >
                        "取消"
                    </button>
                    <button
                        type="submit"
                        class=(class!(BUTTON, PRIMARY))
                        :disabled=$(busy.get())
                    >
                        "关联节点"
                    </button>
                </footer>
            </form>
        )
    })
}
