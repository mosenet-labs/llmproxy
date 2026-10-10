use super::*;
use llmproxy_store::organizations::OrganizationRole;
use serde::Deserialize;
use topcoat::{
    context::app_context,
    router::{content::Form, response::Response, route},
    runtime::procedure,
};

#[derive(Deserialize)]
struct Action {
    csrf: String,
    action: String,
    #[serde(default)]
    space_id: i64,
    #[serde(default)]
    id: i64,
    #[serde(default)]
    user_id: i64,
    #[serde(default)]
    version: u64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    email: String,
    #[serde(default)]
    role: String,
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    revoke_keys: bool,
}

#[procedure("/ui/_topcoat/runtime/procedures/organization-action")]
pub async fn organization_action(cx: &Cx, payload: String) -> Result<auth::Outcome> {
    let Form(input) = Form::<Action>::from_bytes(payload.as_bytes())?;
    crate::app::check_csrf(cx, &input.csrf)?;
    let actor = auth::session(cx)?.user.id;
    let store = &app_context::<crate::app::AppState>(cx).store;
    let role = || match input.role.as_str() {
        "admin" => Ok(OrganizationRole::Admin),
        "member" => Ok(OrganizationRole::Member),
        _ => Err(llmproxy_store::StoreError::Validation(
            "请选择成员角色".into(),
        )),
    };
    let result = async {
        match input.action.as_str() {
            "create" => {
                store.create_organization(actor, &input.name).await?;
            }
            "update" => {
                store
                    .update_organization(
                        actor,
                        input.space_id,
                        input.version,
                        &input.name,
                        input.enabled,
                    )
                    .await?
            }
            "invite" => {
                store
                    .invite_organization_member(actor, input.space_id, &input.email, role()?)
                    .await?
            }
            "accept" => {
                store
                    .accept_organization_invitation(actor, input.id, input.version)
                    .await?
            }
            "revoke-invite" => {
                store
                    .revoke_organization_invitation(actor, input.space_id, input.id, input.version)
                    .await?
            }
            "member" => {
                let Form(fields) = Form::<Vec<(String, String)>>::from_bytes(payload.as_bytes())
                    .map_err(|_| llmproxy_store::StoreError::Validation("表单无效".into()))?;
                let groups = fields
                    .iter()
                    .filter(|(k, _)| k == "group_id")
                    .map(|(_, v)| {
                        v.parse::<i64>()
                            .map_err(|_| llmproxy_store::StoreError::NotFound)
                    })
                    .collect::<llmproxy_store::StoreResult<Vec<_>>>()?;
                store
                    .update_organization_member(
                        actor,
                        input.space_id,
                        input.id,
                        input.version,
                        role()?,
                        groups,
                    )
                    .await?;
            }
            "remove" => {
                store
                    .remove_organization_member(
                        actor,
                        input.space_id,
                        input.id,
                        input.version,
                        input.revoke_keys,
                    )
                    .await?
            }
            "transfer" => {
                store
                    .transfer_organization(actor, input.space_id, input.id, input.version)
                    .await?
            }
            "revoke-keys" => {
                store
                    .revoke_keys_by_creator(actor, input.space_id, input.user_id)
                    .await?
            }
            _ => return Err(llmproxy_store::StoreError::Validation("操作无效".into())),
        }
        Ok::<_, llmproxy_store::StoreError>("已保存，请在顶部切换空间或继续管理".to_owned())
    }
    .await;
    Ok(result.map_err(auth::message))
}

#[derive(Deserialize)]
pub struct SelectSpace {
    csrf: String,
    space_id: i64,
}

#[route(POST "/ui/organizations/select")]
pub async fn select(cx: &Cx, Form(input): Form<SelectSpace>) -> Result<Response> {
    crate::app::check_csrf(cx, &input.csrf)?;
    let store = &app_context::<crate::app::AppState>(cx).store;
    let spaces = store.list_spaces(auth::session(cx)?.user.id).await?;
    let space = spaces
        .iter()
        .find(|s| s.id == input.space_id)
        .ok_or_else(topcoat::router::error::forbidden)?;
    let destination = if space.enabled {
        "/ui/chat"
    } else {
        "/ui/organizations/detail"
    };
    let mut response = Response::new(topcoat::router::Body::empty());
    *response.status_mut() = 303u16.try_into()?;
    response.headers_mut().insert(
        "location",
        format!("{destination}?space={}", space.id).parse()?,
    );
    Ok(response)
}
