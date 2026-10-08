mod app;
mod assets;
mod chat_stream;
pub mod observability;

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use llmproxy_store::ProviderStore;
use topcoat::{
    asset::RouterBuilderAssetExt,
    router::{BodyLimit, Router},
    runtime::{PrefetchMode, RouterBuilderRuntimeExt},
};
use topcoat_ant_design::RouterBuilderUiExt;

pub use topcoat::router::{Body, request::Request, response::Response};

pub use topcoat::runtime::RUNTIME_PROTOCOL;

pub const BODY_LIMIT: usize = 2 * 1024 * 1024;
pub type SubscriptionPresence = std::sync::Arc<
    std::sync::Mutex<std::collections::HashMap<String, llmproxy_core::subscription::Presence>>,
>;

#[derive(Clone)]
pub struct Console {
    history_auth: String,
    router: Router,
    subscriptions: SubscriptionPresence,
}

impl Console {
    pub async fn connect(
        database_url: &str,
        master_key: &str,
        listen: SocketAddr,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let store = ProviderStore::connect(database_url, master_key).await?;
        Self::from_store(store, listen).await
    }

    /// 与 Gateway 共用连接池及历史写锁；只在单个本地服务启动时恢复遗留记录。
    pub async fn from_store(
        store: ProviderStore,
        listen: SocketAddr,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        store.list().await?;
        store.recover_chat_history().await?;
        let mut random = [0u8; 32];
        getrandom::fill(&mut random).map_err(|_| io::Error::other("无法生成安全随机令牌"))?;
        let csrf = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let history_auth = app::chat_service::new_id()?;
        let subscriptions = SubscriptionPresence::default();
        let websocket = match std::env::var("LLMPROXY_UI_WEBSOCKET").as_deref() {
            Ok("true") | Err(std::env::VarError::NotPresent) => true,
            Ok("false") => false,
            _ => return Err(io::Error::other("LLMPROXY_UI_WEBSOCKET 须为 true 或 false").into()),
        };
        let state = app::AppState {
            websocket,
            subscriptions: subscriptions.clone(),
            history_auth: history_auth.clone(),
            store,
            prober: llmproxy_probe::ModelProber::new()?,
            csrf,
            port: listen.port(),
            telemetry: observability::ConsoleTelemetry::new(),
            chat_sessions: std::sync::Arc::new(app::chat_sessions::ChatSessions::default()),
            chat_client: reqwest::Client::builder().no_proxy().build()?,
            gateway_origin: format!(
                "http://{}",
                SocketAddr::new(
                    match listen.ip() {
                        IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
                        IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
                        ip => ip,
                    },
                    listen.port(),
                )
            ),
        };
        let mut builder = app::route_builder()
            .app_context(state)
            .layer(BodyLimit::max(BODY_LIMIT))
            .layer(app::protect)
            .layer(observability::request_layer())
            .route(app::ui::chat::chat_history)
            .route(app::ui::chat::chat_usage)
            .route(app::ui::chat::chat_protocol_picker)
            .route(app::ui::chat::chat_thinking_picker)
            .route(app::ui::chat::chat_session_list)
            .route(app::ui::chat::chat_session_activity)
            .route(app::ui::chat::begin_chat)
            .route(app::ui::chat::send_chat)
            .route(app::ui::chat::chat_stop_button)
            .route(app::ui::chat::stop_chat)
            .route(app::ui::chat::new_chat)
            .route(app::ui::chat::open_chat)
            .route(app::ui::chat::change_chat_history)
            .route(app::ui::chat::retry_chat_save)
            .route(app::ui::chat::switch_chat)
            .route(app::ui::chat::set_chat_thinking)
            .route(app::ui::chat::default_protocol)
            .route(app::ui::routes::save_route)
            .route(app::ui::routes::delete_route)
            .route(app::ui::models::model_workspace)
            .route(app::ui::models::model_candidates)
            .route(app::ui::models::model_draft)
            .route(app::ui::models::load_model_candidates)
            .route(app::ui::models::add_draft_model)
            .route(app::ui::models::remove_draft_model)
            .route(app::ui::models::save_model)
            .route(app::ui::models::save_models)
            .route(app::ui::models::delete_model)
            .route(app::ui::models::probe_saved_model)
            .route(app::ui::models::price_rule_rows)
            .route(app::ui::models::price_matrix_rows)
            .route(app::ui::models::price_peak_windows)
            .route(app::ui::models::add_price_rule)
            .route(app::ui::models::remove_price_rule)
            .route(app::ui::models::add_price_window)
            .route(app::ui::models::remove_price_window)
            .route(app::ui::models::deepseek_peak_windows)
            .route(app::ui::models::preview_price_band)
            .route(app::ui::models::save_price_plan)
            .route(app::ui::holidays::holiday_workspace)
            .route(app::ui::holidays::shift_holiday_date)
            .route(app::ui::holidays::import_holidays)
            .route(app::ui::providers::save_provider)
            .route(app::ui::providers::provider_action)
            .route(app::ui::providers::preview_models)
            .route(app::ui::providers::provider_list)
            .page(app::ui::subscriptions::import_models)
            .route(app::ui::subscriptions::set_enabled)
            .route(app::ui::subscriptions::rename)
            .route(app::ui::subscriptions::save_import)
            .route(assets::component_css)
            .route(assets::console_css)
            .route(assets::runtime_js)
            .route(assets::chat_resume_js)
            .runtime()
            .layer(app::ConnectionGuard)
            .prefetch(PrefetchMode::Intent)
            .max_runs_per_connection(64)
            .topcoat_ant_design()
            .assets(assets::config().map_err(|_| io::Error::other("无法加载控制台静态资源"))?);

        // The framework font route stays internal; its public URL is under /ui.
        *builder
            .get_app_context_mut::<topcoat::font::FontResolver>()
            .expect("registered font resolver") =
            topcoat::font::FontResolver::new(Box::new(|font, writer| {
                use topcoat::router::Route;
                write!(writer, "/ui{}", topcoat::font::FontRoute::new(font).path())
            }));
        Ok(Self {
            history_auth,
            router: builder.build(),
            subscriptions,
        })
    }

    pub fn subscription_presence(&self) -> SubscriptionPresence {
        self.subscriptions.clone()
    }

    /// 内部历史请求须通过服务端令牌认证，不使用浏览器 CSRF 令牌。
    pub fn accepts_history_auth(&self, supplied: &[u8]) -> bool {
        let expected = self.history_auth.as_bytes();
        expected.len() == supplied.len()
            && expected
                .iter()
                .zip(supplied)
                .fold(0, |acc, (a, b)| acc | (a ^ b))
                == 0
    }

    pub async fn handle(&self, mut request: Request) -> Response {
        // Topcoat's font route has a fixed internal path. Keep its public URL under /ui.
        if let Some(path) = request
            .uri()
            .path_and_query()
            .and_then(|path| path.as_str().strip_prefix("/ui/_topcoat/fonts/"))
        {
            *request.uri_mut() = format!("/_topcoat/fonts/{path}")
                .parse()
                .expect("valid prefixed font URI");
        }
        self.router.handle(request).await
    }
}
