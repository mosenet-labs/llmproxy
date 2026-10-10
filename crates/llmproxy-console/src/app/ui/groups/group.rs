use super::*;
use topcoat::router::{module_param, path_param};
use topcoat_ant_design::{tabs, tabs_content, tabs_list, tabs_trigger};

module_param!(pub(super) group_id: i64, error = not_found);

#[derive(Deserialize)]
pub struct DetailQuery {
    #[serde(default)]
    tab: String,
}

#[page]
pub async fn details(Form(query): Form<DetailQuery>) -> Result<impl View> {
    Ok(view! { workspace(initial_tab: query.tab.as_str()) })
}

// Keep existing bookmarks opening the same workspace with the requested panel selected.
#[page("./models")]
pub async fn group_models() -> Result<impl View> {
    Ok(view! { workspace(initial_tab: "models") })
}

#[page("./keys")]
pub async fn keys() -> Result<impl View> {
    Ok(view! { workspace(initial_tab: "keys") })
}

pub(crate) async fn title(cx: &Cx) -> Result<String> {
    let group_id = *path_param::<GroupId>(cx)?;
    Ok(format!("{}详情", find_group(cx, group_id).await?.name))
}

#[component]
async fn workspace(cx: &Cx, initial_tab: &str) -> Result<impl View> {
    crate::app::request_connection(cx);
    let controls = Controls::new(cx);
    let _revision = controls.refresh.get();
    let selected = signal(cx, || {
        if initial_tab == "keys" {
            "keys"
        } else {
            "models"
        }
        .to_owned()
    });
    let secret = signal(cx, String::new);
    let created = signal(cx, || false);
    let group_id = *path_param::<GroupId>(cx)?;
    let group = find_group(cx, group_id).await?;
    let store = crate::app::store(cx).for_group(group_id);
    let routes = store.list_routes().await?;
    let models = store.list_models().await?;
    let csrf = crate::app::auth::csrf_token(cx);
    let refresh = controls.refresh.clone();
    let success = controls.success.clone();
    let failure = controls.failure.clone();
    Ok(view! {
        feedback(controls: &controls)
        group_heading(group: &group)
        tabs(attrs: attributes! { aria-label="资源组详情" },
            tabs_list(attrs: attributes! { role="navigation" aria-label="资源组分类" },
                tabs_trigger(active: $(selected.get() == "models"), attrs: attributes! { cx =>
                    id="group-tab-models" aria-controls="group-models-panel"
                    href=(crate::app::scoped_href(cx, href!(details, GroupId(group_id)) .resolve(cx)))
                    @click=$(|event: Event| { event.prevent_default(); selected.set("models".to_owned()); })
                }, "Models")
                tabs_trigger(active: $(selected.get() == "keys"), attrs: attributes! { cx =>
                    id="group-tab-keys" aria-controls="group-keys-panel"
                    href=(crate::app::scoped_href(cx, href!(details, GroupId(group_id)).query([("tab", "keys")]) .resolve(cx)))
                    @click=$(|event: Event| { event.prevent_default(); selected.set("keys".to_owned()); })
                }, "Keys")
            )
            tabs_content(attrs: attributes! { cx => id="group-models-panel" aria-labelledby="group-tab-models" :hidden=$(selected.get() != "models") },
                super::resources::resource_workspace(group_id: group_id, controls: &controls)
            )
            tabs_content(attrs: attributes! { cx => id="group-keys-panel" aria-labelledby="group-tab-keys" :hidden=$(selected.get() != "keys") },
                key_workspace(
                    group_id: group_id,
                    revision: $(refresh.get()),
                    refresh: $(refresh), success: $(success), failure: $(failure)
                )
            )
        )
        key_editor(group_id: store.group_id(), models: &models, routes: &routes, controls: &controls, csrf: csrf.as_str(), secret: &secret, created: &created)
        secret_dialog(secret: &secret, open: &created)
    })
}
