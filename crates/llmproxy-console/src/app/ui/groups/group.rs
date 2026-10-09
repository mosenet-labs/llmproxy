use super::*;
use topcoat::router::{module_param, path_param};

module_param!(pub(super) group_id: i64, error = not_found);

#[page("./models")]
pub async fn group_models(cx: &Cx) -> Result<impl View> {
    crate::app::request_connection(cx);
    let group_id = *path_param::<GroupId>(cx)?;
    let controls = Controls::new(cx);
    let _revision = controls.refresh.get();
    Ok(view! {
        feedback(controls: &controls)
        super::resources::resource_workspace(group_id: group_id, controls: &controls)
    })
}

#[page("./keys")]
pub async fn keys(cx: &Cx) -> Result<impl View> {
    let controls = Controls::new(cx);
    let secret = signal(cx, String::new);
    let created = signal(cx, || false);
    let group_id = *path_param::<GroupId>(cx)?;
    find_group(cx, group_id).await?;
    let store = app_context::<AppState>(cx).store.for_group(group_id);
    let routes = store.list_routes().await?;
    let models = store.list_models().await?;
    let csrf = app_context::<AppState>(cx).csrf.clone();
    let refresh = controls.refresh.clone();
    let success = controls.success.clone();
    let failure = controls.failure.clone();
    Ok(view! {
        feedback(controls: &controls)
        key_workspace(
            group_id: group_id,
            revision: $(refresh.get()),
            refresh: $(refresh), success: $(success), failure: $(failure)
        )
        key_editor(group_id: store.group_id(), models: &models, routes: &routes, controls: &controls, csrf: csrf.as_str(), secret: &secret, created: &created)
        secret_dialog(secret: &secret, open: &created)
    })
}
