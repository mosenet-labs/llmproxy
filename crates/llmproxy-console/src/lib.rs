mod app;
mod assets;
pub mod observability;

use std::io;

use llmproxy_store::ProviderStore;
use topcoat::{
    asset::RouterBuilderAssetExt,
    router::{BodyLimit, Router},
    runtime::{RouterBuilderProcedureExt, RouterBuilderRuntimeExt, RouterBuilderShardExt},
};
use topcoat_ant_design::RouterBuilderUiExt;

pub use topcoat::router::{Body, request::Request, response::Response};

pub const BODY_LIMIT: usize = 32 * 1024;

pub struct Console {
    router: Router,
}

impl Console {
    pub async fn connect(
        database_url: &str,
        master_key: &str,
        port: u16,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let store = ProviderStore::connect(database_url, master_key).await?;
        store.list().await?;
        let mut random = [0u8; 32];
        getrandom::fill(&mut random).map_err(|_| io::Error::other("无法生成安全随机令牌"))?;
        let csrf = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let state = app::AppState {
            store,
            csrf,
            port,
            telemetry: observability::ConsoleTelemetry::new(),
        };
        let mut builder = Router::builder()
            .app_context(state)
            .layer(BodyLimit::max(BODY_LIMIT))
            .layer(app::protect)
            .layer(observability::request_layer())
            .layout(app::shell)
            .page(app::list)
            .page(app::routes)
            .route(app::providers_redirect)
            .page(app::form)
            .route(app::save)
            .route(app::perform_action)
            .procedure(app::save_provider)
            .procedure(app::provider_action)
            .procedure(app::preview_models)
            .shard(app::provider_list)
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
        // Adapt only Topcoat's fixed internal endpoints. Page URLs already carry /ui.
        if let Some(path) = request
            .uri()
            .path_and_query()
            .and_then(|path| path.as_str().strip_prefix("/ui/_topcoat/"))
        {
            *request.uri_mut() = format!("/_topcoat/{path}")
                .parse()
                .expect("valid prefixed request URI");
        }
        self.router.handle(request).await
    }
}
