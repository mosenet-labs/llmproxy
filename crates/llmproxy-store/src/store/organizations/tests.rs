use super::*;
use crate::{
    ModelRouteTargetInput, ProviderPaths, VirtualKeyInput,
    auth::{EmailPurpose, UserRole},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use sha2::Digest;

static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
const PASSWORD: &str = "organization-test-password";

#[tokio::test]
async fn default_organization_keeps_new_admin_chats_private_and_legacy_history_readable() {
    let (store, directory) = fixture().await;
    let first = store
        .bootstrap_admin("first-admin@example.test", PASSWORD)
        .await
        .unwrap();
    let second = user(&store, "second-admin@example.test").await;
    let second_user = store.get_user(second).await.unwrap();
    store
        .update_user(
            first.id,
            second,
            second_user.version,
            &second_user.display_name,
            UserRole::Admin,
            true,
        )
        .await
        .unwrap();
    let selection = llmproxy_core::conversation::Selection {
        model_id: String::new(),
        protocol: Protocol::OpenAiChat,
    };
    let legacy = "c".repeat(32);
    store
        .create_chat_conversation(&legacy, &selection)
        .await
        .unwrap();
    let first = store.for_space(first.id, 1).await.unwrap();
    let second = store.for_space(second, 1).await.unwrap();
    let first_id = "a".repeat(32);
    let second_id = "b".repeat(32);
    first
        .create_chat_conversation(&first_id, &selection)
        .await
        .unwrap();
    second
        .create_chat_conversation(&second_id, &selection)
        .await
        .unwrap();
    assert!(first.get_chat_conversation(&second_id).await.is_err());
    assert!(second.get_chat_conversation(&first_id).await.is_err());
    for scoped in [&first, &second] {
        assert!(scoped.get_chat_conversation(&legacy).await.is_ok());
        assert_eq!(
            scoped.list_chat_conversations(0, 10).await.unwrap().len(),
            2
        );
    }
    assert!(store.get_chat_conversation(&first_id).await.is_ok());
    drop((first, second, store));
    std::fs::remove_dir_all(directory).unwrap();
}

async fn fixture() -> (ProviderStore, std::path::PathBuf) {
    let directory = std::env::temp_dir().join(format!(
        "llmproxy-organizations-{}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir(&directory).unwrap();
    let store = ProviderStore::connect(
        &format!("sqlite:{}", directory.join("db.sqlite").display()),
        &STANDARD.encode([7; 32]),
    )
    .await
    .unwrap();
    store.migrate().await.unwrap();
    (store, directory)
}
async fn user(store: &ProviderStore, email: &str) -> i64 {
    let code = store
        .issue_email_code(email, EmailPurpose::Register)
        .await
        .unwrap()
        .unwrap();
    store
        .register_user(email, &code.code, PASSWORD, "")
        .await
        .unwrap();
    store
        .login(email, PASSWORD)
        .await
        .unwrap()
        .unwrap()
        .session
        .user
        .id
}
fn provider() -> ProviderInput {
    ProviderInput {
        name: "same-provider".into(),
        paths: ProviderPaths::single(Protocol::OpenAiChat),
        host: "example.com".into(),
        port: 443,
        tls: true,
        api_key: "org-secret".into(),
        enabled: true,
        models_path: "/models".into(),
        models_protocol: Protocol::OpenAiChat,
        anthropic_version: None,
        messages_auth: MessagesAuth::ApiKey,
        connect_timeout_ms: 1000,
        read_timeout_ms: 1000,
        write_timeout_ms: 1000,
    }
}
async fn add_model(store: &ProviderStore) -> ModelMappingView {
    let provider = store.create(provider()).await.unwrap();
    store
        .create_model(ModelMappingInput {
            alias: "same-model".into(),
            provider_id: provider.id,
            upstream_model_id: "upstream".into(),
            protocols: vec![Protocol::OpenAiChat],
            reference_price: None,
            thinking: Default::default(),
        })
        .await
        .unwrap()
}
async fn resources(store: &ProviderStore, model: i64, routes: Vec<i64>) {
    let version = store
        .list_groups()
        .await
        .unwrap()
        .into_iter()
        .find(|g| g.id == store.group_id())
        .unwrap()
        .version;
    store
        .set_group_resources(version, vec![model], routes)
        .await
        .unwrap();
}
async fn make_key(store: &ProviderStore) -> crate::CreatedVirtualKey {
    store
        .create_virtual_key(VirtualKeyInput {
            name: "application".into(),
            all_routes: true,
            route_ids: vec![],
            model_ids: vec![],
            expires_at: None,
        })
        .await
        .unwrap()
}
async fn accept_invite(
    store: &ProviderStore,
    owner: i64,
    space: i64,
    recipient: i64,
    email: &str,
    role: OrganizationRole,
) {
    store
        .invite_organization_member(owner, space, email, role)
        .await
        .unwrap();
    let invitation = store
        .organization_invitations(recipient, None)
        .await
        .unwrap()
        .remove(0);
    assert!(
        store
            .accept_organization_invitation(owner, invitation.id, invitation.version)
            .await
            .is_err()
    );
    store
        .accept_organization_invitation(recipient, invitation.id, invitation.version)
        .await
        .unwrap();
    assert!(
        store
            .accept_organization_invitation(recipient, invitation.id, invitation.version)
            .await
            .is_err()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spaces_isolate_catalogs_members_history_and_keys() {
    let (store, directory) = fixture().await;
    let owner = user(&store, "owner@example.test").await;
    let member = user(&store, "member@example.test").await;
    let outsider = user(&store, "outsider@example.test").await;
    let first = store.create_organization(owner, "First").await.unwrap();
    let second = store.create_organization(owner, "Second").await.unwrap();
    let personal = store.for_user(owner).await.unwrap();
    let first_store = store.for_space(owner, first.id).await.unwrap();
    let second_store = store.for_space(owner, second.id).await.unwrap();
    let m1 = add_model(&first_store).await;
    let m2 = add_model(&second_store).await;
    let mp = add_model(&personal).await;
    assert!(first_store.list_models().await.unwrap().is_empty());
    assert_eq!(first_store.list_all_models().await.unwrap().len(), 1);
    for id in [m2.id, mp.id] {
        assert!(first_store.get_model(id).await.is_err());
    }
    let r1 = first_store
        .create_route(ModelRouteInput {
            name: "route".into(),
            protocol: Protocol::OpenAiChat,
            provider_protocol: Protocol::OpenAiChat,
            enabled: true,
            targets: vec![ModelRouteTargetInput {
                model_id: m1.id,
                enabled: true,
            }],
        })
        .await
        .unwrap();
    assert!(
        second_store
            .create_route(ModelRouteInput {
                name: "route".into(),
                protocol: Protocol::OpenAiChat,
                provider_protocol: Protocol::OpenAiChat,
                enabled: true,
                targets: vec![ModelRouteTargetInput {
                    model_id: m1.id,
                    enabled: true
                }]
            })
            .await
            .is_err()
    );
    resources(&first_store, m1.id, vec![r1.id]).await;
    let group = first_store.create_group("Another").await.unwrap();
    resources(&first_store.for_group(group.id), m1.id, vec![r1.id]).await;
    let key = make_key(&first_store).await;
    let identity = store
        .authenticate_virtual_key(&key.secret)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(identity.space_id, first.id);
    assert_eq!(key.view.created_by_user_id, Some(owner));
    assert!(
        second_store
            .for_group(first_store.group_id())
            .list_models()
            .await
            .is_err()
    );
    assert!(store.for_space(outsider, first.id).await.is_err());
    accept_invite(
        &store,
        owner,
        first.id,
        member,
        "member@example.test",
        OrganizationRole::Member,
    )
    .await;
    let read_only = store.for_space(member, first.id).await.unwrap();
    assert!(read_only.list_groups().await.unwrap().is_empty());
    assert!(read_only.list_all_models().await.is_err());
    assert!(read_only.probe_target(m1.provider_id).await.is_err());
    assert!(read_only.create_group("forbidden").await.is_err());
    let member_row = store
        .organization_members(owner, first.id)
        .await
        .unwrap()
        .into_iter()
        .find(|m| m.user_id == member)
        .unwrap();
    assert!(
        store
            .update_organization_member(
                owner,
                first.id,
                member_row.id,
                member_row.version,
                OrganizationRole::Member,
                vec![second_store.group_id()]
            )
            .await
            .is_err()
    );
    store
        .update_organization_member(
            owner,
            first.id,
            member_row.id,
            member_row.version,
            OrganizationRole::Member,
            vec![first_store.group_id()],
        )
        .await
        .unwrap();
    let read_only = store.for_space(member, first.id).await.unwrap();
    assert_eq!(read_only.list_groups().await.unwrap().len(), 1);
    assert_eq!(read_only.list_models().await.unwrap().len(), 1);
    assert_eq!(read_only.list_routes().await.unwrap().len(), 1);
    assert!(read_only.for_group(group.id).list_models().await.is_err());
    assert!(read_only.list_virtual_keys().await.is_err());
    assert!(
        read_only
            .set_virtual_key_enabled(key.view.id, key.view.version, false)
            .await
            .is_err()
    );
    assert!(
        read_only
            .create_virtual_key(VirtualKeyInput {
                name: "forbidden".into(),
                all_routes: true,
                route_ids: vec![],
                model_ids: vec![],
                expires_at: None
            })
            .await
            .is_err()
    );
    let selection = llmproxy_core::conversation::Selection {
        model_id: m1.id.to_string(),
        protocol: Protocol::OpenAiChat,
    };
    let conversation = "a".repeat(32);
    first_store
        .create_chat_conversation(&conversation, &selection)
        .await
        .unwrap();
    assert!(
        read_only
            .get_chat_conversation(&conversation)
            .await
            .is_err()
    );
    first_store
        .begin_chat_turn(crate::chat_history::Input {
            id: "c".repeat(32),
            conversation_id: conversation.clone(),
            selection: selection.clone(),
            alias: m1.alias.clone(),
            thinking: Default::default(),
            prompt: "hello".into(),
        })
        .await
        .unwrap();
    store
        .for_group(first_store.group_id())
        .recover_chat_history()
        .await
        .unwrap();
    assert_eq!(
        first_store.chat_turn(&"c".repeat(32)).await.unwrap().status,
        crate::chat_history::Status::Interrupted
    );
    assert!(
        read_only
            .list_chat_conversations(0, 10)
            .await
            .unwrap()
            .is_empty()
    );
    read_only
        .create_chat_conversation(&"b".repeat(32), &selection)
        .await
        .unwrap();
    let member_row = store
        .organization_members(owner, first.id)
        .await
        .unwrap()
        .into_iter()
        .find(|m| m.user_id == member)
        .unwrap();
    store
        .update_organization_member(
            owner,
            first.id,
            member_row.id,
            member_row.version,
            OrganizationRole::Member,
            vec![],
        )
        .await
        .unwrap();
    assert!(read_only.list_models().await.is_err());
    assert!(
        read_only
            .get_chat_conversation(&"b".repeat(32))
            .await
            .is_err()
    );
    store
        .update_organization(owner, first.id, first.version, &first.name, false)
        .await
        .unwrap();
    assert!(
        store
            .authenticate_virtual_key(&key.secret)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        !store
            .for_group(first_store.group_id())
            .group_enabled()
            .await
            .unwrap()
    );
    assert!(first_store.create_group("disabled").await.is_err());
    let current = store
        .for_space(owner, first.id)
        .await
        .unwrap()
        .current_space()
        .await
        .unwrap()
        .unwrap();
    store
        .update_organization(owner, first.id, current.version, &first.name, true)
        .await
        .unwrap();
    assert!(
        store
            .authenticate_virtual_key(&key.secret)
            .await
            .unwrap()
            .is_some()
    );
    let member_row = store
        .organization_members(owner, first.id)
        .await
        .unwrap()
        .into_iter()
        .find(|m| m.user_id == member)
        .unwrap();
    store
        .remove_organization_member(owner, first.id, member_row.id, member_row.version, false)
        .await
        .unwrap();
    assert!(read_only.list_groups().await.is_err());
    assert!(
        store
            .authenticate_virtual_key(&key.secret)
            .await
            .unwrap()
            .is_some()
    );
    drop((store, personal, first_store, second_store, read_only));
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn organization_keys_survive_creator_departure_and_owner_transfer_is_atomic() {
    let (store, directory) = fixture().await;
    let platform = store
        .bootstrap_admin("platform@example.test", PASSWORD)
        .await
        .unwrap();
    assert_eq!(
        store
            .list_spaces(platform.id)
            .await
            .unwrap()
            .iter()
            .find(|s| s.id == 1)
            .unwrap()
            .role,
        OrganizationRole::Owner
    );
    let owner = user(&store, "owner@example.test").await;
    let admin = user(&store, "admin@example.test").await;
    let space = store.create_organization(owner, "Team").await.unwrap();
    accept_invite(
        &store,
        owner,
        space.id,
        admin,
        "admin@example.test",
        OrganizationRole::Admin,
    )
    .await;
    let resources = store.for_space(admin, space.id).await.unwrap();
    let key = make_key(&resources).await;
    let personal_key = make_key(&store.for_user(admin).await.unwrap()).await;
    let current = store.get_user(admin).await.unwrap();
    store
        .update_user(
            platform.id,
            admin,
            current.version,
            &current.display_name,
            UserRole::User,
            false,
        )
        .await
        .unwrap();
    assert!(
        store
            .authenticate_virtual_key(&key.secret)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        store
            .authenticate_virtual_key(&personal_key.secret)
            .await
            .unwrap()
            .is_none()
    );
    let current = store.get_user(admin).await.unwrap();
    store
        .update_user(
            platform.id,
            admin,
            current.version,
            &current.display_name,
            UserRole::User,
            true,
        )
        .await
        .unwrap();
    let target = store
        .organization_members(owner, space.id)
        .await
        .unwrap()
        .into_iter()
        .find(|m| m.user_id == admin)
        .unwrap();
    assert!(
        store
            .transfer_organization(admin, space.id, target.id, target.version)
            .await
            .is_err()
    );
    store
        .transfer_organization(owner, space.id, target.id, target.version)
        .await
        .unwrap();
    assert!(
        store
            .transfer_organization(owner, space.id, target.id, target.version)
            .await
            .is_err()
    );
    let members = store.organization_members(admin, space.id).await.unwrap();
    assert_eq!(
        members
            .iter()
            .filter(|m| m.role == OrganizationRole::Owner)
            .count(),
        1
    );
    let previous = members.iter().find(|m| m.user_id == owner).unwrap();
    assert_eq!(previous.role, OrganizationRole::Admin);
    assert!(
        store
            .remove_organization_member(admin, space.id, target.id, target.version, false)
            .await
            .is_err()
    );
    store
        .remove_organization_member(admin, space.id, previous.id, previous.version, false)
        .await
        .unwrap();
    assert!(store.for_space(owner, space.id).await.is_err());
    assert!(
        store
            .authenticate_virtual_key(&key.secret)
            .await
            .unwrap()
            .is_some()
    );
    store
        .revoke_keys_by_creator(admin, space.id, admin)
        .await
        .unwrap();
    assert!(
        store
            .authenticate_virtual_key(&key.secret)
            .await
            .unwrap()
            .is_none()
    );
    drop((store, resources));
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn upgrade_preserves_personal_and_platform_resources_and_virtual_key_secrets() {
    let directory = std::env::temp_dir().join(format!(
        "llmproxy-org-upgrade-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let url = format!("sqlite:{}", directory.join("db.sqlite").display());
    let mut db = Db::builder().max_pool_size(1).connect(&url).await.unwrap();
    toasty::sql::query("PRAGMA foreign_keys=OFF")
        .exec(&mut db)
        .await
        .unwrap();
    let migrations = SQLITE_MIGRATIONS.migrations();
    let previous = toasty::migration::MigrationSet::new(Box::leak(
        migrations[..migrations.len() - 1]
            .to_vec()
            .into_boxed_slice(),
    ));
    previous.apply(&db).await.unwrap();
    toasty::sql::statement("INSERT INTO users(id,email,display_name,password_hash,role,enabled,email_verified,created_at,updated_at) VALUES (1,'admin@example.test','Admin','unused','admin',TRUE,TRUE,1,1),(2,'personal@example.test','User','unused','user',TRUE,TRUE,1,1)").exec(&mut db).await.unwrap();
    let cipher = KeyCipher::new(&STANDARD.encode([7; 32])).unwrap();
    let encrypted = cipher.encrypt("preserved-provider-secret").unwrap();
    toasty::sql::statement(format!("INSERT INTO providers(id,name,host,port,tls,encrypted_key,enabled,connect_timeout_ms,read_timeout_ms,write_timeout_ms,updated_at,models_path,models_protocol,models_probe_status,messages_auth,openai_chat_path,owner_user_id) VALUES (1,'platform','example.com',443,TRUE,'{encrypted}',TRUE,1000,1000,1000,1,'/models','openai_chat','unprobed','x-api-key','/v1/chat/completions',NULL),(2,'personal','example.com',443,TRUE,'{encrypted}',TRUE,1000,1000,1000,1,'/models','openai_chat','unprobed','x-api-key','/v1/chat/completions',2)")).exec(&mut db).await.unwrap();
    toasty::sql::statement("INSERT INTO groups(id,name,enabled,updated_at,owner_user_id) VALUES(2,'personal-group',TRUE,1,2)").exec(&mut db).await.unwrap();
    let secret = format!(
        "lp-vk-{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([8; 32])
    );
    toasty::sql::statement(format!("INSERT INTO virtual_keys(id,group_id,name,digest,prefix,all_routes,route_ids,model_ids,enabled,revoked,created_at) VALUES(1,2,'preserved','{}','{}',TRUE,'[]','[]',TRUE,FALSE,1)",format_args!("{:x}",sha2::Sha256::digest(&secret)),&secret[..14])).exec(&mut db).await.unwrap();
    drop(db);
    let store = ProviderStore::connect(&url, &STANDARD.encode([7; 32]))
        .await
        .unwrap();
    store.migrate().await.unwrap();
    let personal = store.for_user(2).await.unwrap();
    assert_eq!(personal.space_id(), Some(3));
    assert_eq!(personal.list().await.unwrap()[0].id, 2);
    assert_eq!(
        personal.probe_target(2).await.unwrap().secret,
        "preserved-provider-secret"
    );
    assert!(personal.get(1).await.is_err());
    assert_eq!(personal.list_groups().await.unwrap()[0].id, 2);
    let identity = store
        .authenticate_virtual_key(&secret)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(identity.space_id, 3);
    assert_eq!(identity.group_id, 2);
    assert_eq!(identity.key_id, 1);
    assert_eq!(
        store.for_space(1, 1).await.unwrap().list().await.unwrap()[0].id,
        1
    );
    assert_eq!(
        store
            .list_spaces(1)
            .await
            .unwrap()
            .iter()
            .find(|s| s.id == 1)
            .unwrap()
            .role,
        OrganizationRole::Owner
    );
    store.migrate().await.unwrap();
    drop((store, personal));
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn expired_invitations_and_foreign_member_changes_are_rejected() {
    let (store, directory) = fixture().await;
    let owner = user(&store, "owner@example.test").await;
    let recipient = user(&store, "recipient@example.test").await;
    let space = store.create_organization(owner, "Team").await.unwrap();
    assert!(
        store
            .invite_organization_member(
                owner,
                space.id,
                "missing@example.test",
                OrganizationRole::Member
            )
            .await
            .is_err()
    );
    store
        .invite_organization_member(
            owner,
            space.id,
            "recipient@example.test",
            OrganizationRole::Admin,
        )
        .await
        .unwrap();
    assert!(
        store
            .invite_organization_member(
                owner,
                space.id,
                "recipient@example.test",
                OrganizationRole::Member
            )
            .await
            .is_err()
    );
    let invite = store
        .organization_invitations(recipient, None)
        .await
        .unwrap()
        .remove(0);
    let mut connection = store.connection().await.unwrap();
    let mut row = OrganizationInvitationRow::filter_by_id(invite.id)
        .get(&mut connection)
        .await
        .unwrap();
    row.update()
        .expires_at(0_i64)
        .exec(&mut connection)
        .await
        .unwrap();
    assert!(
        store
            .accept_organization_invitation(recipient, invite.id, row.version)
            .await
            .is_err()
    );
    drop(connection);
    accept_invite(
        &store,
        owner,
        space.id,
        recipient,
        "recipient@example.test",
        OrganizationRole::Admin,
    )
    .await;
    let recipient_store = store.for_space(recipient, space.id).await.unwrap();
    let application = make_key(&recipient_store).await;
    assert!(
        store
            .update_organization(recipient, space.id, space.version, "Renamed", true)
            .await
            .is_err()
    );
    let target = store
        .organization_members(owner, space.id)
        .await
        .unwrap()
        .into_iter()
        .find(|m| m.user_id == recipient)
        .unwrap();
    assert!(
        store
            .remove_organization_member(owner, space.id, target.id, target.version + 1, true)
            .await
            .is_err()
    );
    assert!(
        store
            .authenticate_virtual_key(&application.secret)
            .await
            .unwrap()
            .is_some()
    );
    store
        .remove_organization_member(owner, space.id, target.id, target.version, true)
        .await
        .unwrap();
    assert!(
        store
            .authenticate_virtual_key(&application.secret)
            .await
            .unwrap()
            .is_none()
    );
    drop((store, recipient_store));
    std::fs::remove_dir_all(directory).unwrap();
}
