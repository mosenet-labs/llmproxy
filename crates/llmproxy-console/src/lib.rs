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
    runtime::RouterBuilderRuntimeExt,
};
use topcoat_ant_design::RouterBuilderUiExt;

pub use topcoat::router::{Body, request::Request, response::Response};

pub const BODY_LIMIT: usize = 2 * 1024 * 1024;

pub struct Console {
    router: Router,
}

impl Console {
    pub async fn connect(
        database_url: &str,
        master_key: &str,
        listen: SocketAddr,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let store = ProviderStore::connect(database_url, master_key).await?;
        store.list().await?;
        let mut random = [0u8; 32];
        getrandom::fill(&mut random).map_err(|_| io::Error::other("无法生成安全随机令牌"))?;
        let csrf = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let state = app::AppState {
            store,
            prober: llmproxy_probe::ModelProber::new()?,
            csrf,
            port: listen.port(),
            telemetry: observability::ConsoleTelemetry::new(),
            chat_sessions: app::chat_sessions::ChatSessions::default(),
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
            .route(app::ui::chat::chat_protocol_picker)
            .route(app::ui::chat::chat_session_list)
            .route(app::ui::chat::begin_chat)
            .route(app::ui::chat::send_chat)
            .route(app::ui::chat::new_chat)
            .route(app::ui::chat::default_protocol)
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
            .route(app::ui::models::price_peak_windows)
            .route(app::ui::models::add_price_rule)
            .route(app::ui::models::remove_price_rule)
            .route(app::ui::models::add_price_window)
            .route(app::ui::models::remove_price_window)
            .route(app::ui::models::save_price_plan)
            .route(app::ui::holidays::holiday_workspace)
            .route(app::ui::holidays::shift_holiday_date)
            .route(app::ui::holidays::import_holidays)
            .route(app::ui::providers::save_provider)
            .route(app::ui::providers::provider_action)
            .route(app::ui::providers::preview_models)
            .route(app::ui::providers::provider_list)
            .route(assets::component_css)
            .route(assets::console_css)
            .route(assets::runtime_js)
            .runtime()
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
            router: builder.build(),
        })
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
