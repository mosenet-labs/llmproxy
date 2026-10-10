#[allow(dead_code)]
mod nonstream;
mod support;

use llmproxy_store::{
    auth::EmailPurpose,
    organizations::{OrganizationRole, SpaceKind},
};
use nonstream::{Database, MASTER_KEY};
use reqwest::{Client, StatusCode, redirect::Policy};
use support::Gateway;
const PASSWORD: &str = "organization-test-password";

async fn account(database: &Database, email: &str) -> (i64, Client, String) {
    let code = database
        .store
        .issue_email_code(email, EmailPurpose::Register)
        .await
        .unwrap()
        .unwrap();
    database
        .store
        .register_user(email, &code.code, PASSWORD, "")
        .await
        .unwrap();
    let login = database
        .store
        .login(email, PASSWORD)
        .await
        .unwrap()
        .unwrap();
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "cookie",
        format!("llmproxy_session={}", login.secret)
            .parse()
            .unwrap(),
    );
    (
        login.session.user.id,
        Client::builder()
            .no_proxy()
            .redirect(Policy::none())
            .default_headers(headers)
            .build()
            .unwrap(),
        login.session.csrf,
    )
}
async fn procedure(
    client: &Client,
    base: &str,
    name: &str,
    page: &str,
    fields: &[(&str, &str)],
) -> reqwest::Response {
    let payload = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(fields.iter().copied())
        .finish();
    client
        .post(format!("{base}/ui/_topcoat/runtime/procedures/{name}"))
        .header("origin", base)
        .header("referer", format!("{base}{page}"))
        .json(&[payload])
        .send()
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn organization_runtime_pins_space_and_enforces_membership() {
    let database = Database::new().await;
    let (owner, owner_client, csrf) = account(&database, "owner@example.test").await;
    let (member, member_client, member_csrf) = account(&database, "member@example.test").await;
    let (_, outsider, _) = account(&database, "outsider@example.test").await;
    let gateway = Gateway::database(&database.url, MASTER_KEY);
    let base = format!("http://{}", gateway.address);
    let response = procedure(
        &owner_client,
        &base,
        "organization-action",
        "/ui/organizations",
        &[
            ("csrf", &csrf),
            ("action", "create"),
            ("name", "Organization A"),
        ],
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.text().await.unwrap().contains("已保存"));
    let spaces = database.store.list_spaces(owner).await.unwrap();
    let space = spaces
        .iter()
        .find(|s| s.kind == SpaceKind::Organization)
        .unwrap();
    let personal = spaces
        .iter()
        .find(|s| s.kind == SpaceKind::Personal)
        .unwrap();
    let space_id = space.id.to_string();
    let page = format!("/ui/groups?space={space_id}");
    let response = owner_client
        .get(format!("{base}{page}"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let html = response.text().await.unwrap();
    assert!(html.contains("id=\"header-space\""));
    assert!(html.contains(&format!("href=\"/ui/providers?space={space_id}\"")));
    assert!(html.contains("id=\"nav-organizations\""));
    assert_eq!(
        outsider
            .get(format!("{base}{page}"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    let selected = owner_client
        .post(format!("{base}/ui/organizations/select"))
        .header("origin", &base)
        .form(&[
            ("csrf", csrf.clone()),
            ("space_id", personal.id.to_string()),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(selected.status(), StatusCode::SEE_OTHER);
    assert!(!selected.headers().contains_key("set-cookie"));
    let response = procedure(
        &owner_client,
        &base,
        "save-group",
        &page,
        &[("csrf", &csrf), ("name", "Original tab")],
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.text().await.unwrap().contains("组已创建"));
    assert!(
        database
            .store
            .for_space(owner, space.id)
            .await
            .unwrap()
            .list_groups()
            .await
            .unwrap()
            .iter()
            .any(|g| g.name == "Original tab")
    );
    assert!(
        !database
            .store
            .for_user(owner)
            .await
            .unwrap()
            .list_groups()
            .await
            .unwrap()
            .iter()
            .any(|g| g.name == "Original tab")
    );
    let detail = format!("/ui/organizations/detail?space={space_id}");
    let response = procedure(
        &owner_client,
        &base,
        "organization-action",
        &detail,
        &[
            ("csrf", &csrf),
            ("action", "invite"),
            ("space_id", &space_id),
            ("email", "member@example.test"),
            ("role", "member"),
        ],
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.text().await.unwrap().contains("已保存"));
    let html = member_client
        .get(format!("{base}/ui/organizations"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains("Organization A"));
    assert!(html.contains("接受邀请"));
    let invite = database
        .store
        .organization_invitations(member, None)
        .await
        .unwrap()
        .remove(0);
    let response = procedure(
        &member_client,
        &base,
        "organization-action",
        "/ui/organizations",
        &[
            ("csrf", &member_csrf),
            ("action", "accept"),
            ("id", &invite.id.to_string()),
            ("version", &invite.version.to_string()),
        ],
    )
    .await;
    assert!(response.text().await.unwrap().contains("已保存"));
    let chat = member_client
        .get(format!("{base}/ui/chat?space={space_id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(chat.status(), StatusCode::OK);
    assert!(chat.text().await.unwrap().contains("尚未获授权的资源组"));
    for path in [
        "/ui/providers",
        "/ui/subscriptions",
        "/ui/keys",
        "/ui/groups",
    ] {
        assert_eq!(
            member_client
                .get(format!("{base}{path}?space={space_id}"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    let rejected = procedure(
        &member_client,
        &base,
        "save-group",
        &format!("/ui/chat?space={space_id}"),
        &[("csrf", &member_csrf), ("name", "Forbidden")],
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::FORBIDDEN);
    let owner_store = database.store.for_space(owner, space.id).await.unwrap();
    let row = database
        .store
        .organization_members(owner, space.id)
        .await
        .unwrap()
        .into_iter()
        .find(|m| m.user_id == member)
        .unwrap();
    database
        .store
        .update_organization_member(
            owner,
            space.id,
            row.id,
            row.version,
            OrganizationRole::Member,
            vec![owner_store.group_id()],
        )
        .await
        .unwrap();
    let models = member_client
        .get(format!("{base}/ui/models?space={space_id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(models.status(), StatusCode::OK);
    let html = models.text().await.unwrap();
    assert!(html.contains("可调用模型"));
    assert!(!html.contains("id=\"nav-providers\""));
    assert!(!html.contains("id=\"nav-keys\""));
    let selected = member_client
        .post(format!("{base}/ui/groups/select"))
        .header("origin", &base)
        .header("referer", format!("{base}/ui/models?space={space_id}"))
        .form(&[
            ("csrf", member_csrf.clone()),
            ("group_id", owner_store.group_id().to_string()),
            ("return_to", "/ui/models".into()),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(selected.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        selected.headers()["location"],
        format!("/ui/models?space={space_id}")
    );
    let response = procedure(
        &owner_client,
        &base,
        "save-group",
        "/ui/groups?space=999999",
        &[("csrf", &csrf), ("name", "Forbidden")],
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
