mod app;
mod assets;

use std::{env, io};

use llmproxy_store::ProviderStore;
use topcoat::{
    asset::RouterBuilderAssetExt,
    router::{BodyLimit, Router},
    runtime::RouterBuilderRuntimeExt,
};
use topcoat_ant_design::RouterBuilderUiExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();
    let database_url = env::var("LLMPROXY_DATABASE_URL")
        .map_err(|_| io::Error::other("请设置 LLMPROXY_DATABASE_URL"))?;
    let master_key = env::var("LLMPROXY_MASTER_KEY")
        .map_err(|_| io::Error::other("请设置 LLMPROXY_MASTER_KEY"))?;
    let port = env::var("LLMPROXY_CONSOLE_PORT")
        .unwrap_or_else(|_| "3200".to_owned())
        .parse::<u16>()
        .map_err(|_| io::Error::other("LLMPROXY_CONSOLE_PORT 必须是有效端口"))?;
    if port == 0 {
        return Err(io::Error::other("LLMPROXY_CONSOLE_PORT 不能为 0").into());
    }
    let store = ProviderStore::connect(&database_url, &master_key).await?;
    store.list().await?;
    let mut random = [0u8; 32];
    getrandom::fill(&mut random).map_err(|_| io::Error::other("无法生成安全随机令牌"))?;
    let csrf = random.iter().map(|byte| format!("{byte:02x}")).collect();
    let state = app::AppState { store, csrf, port };
    let router = Router::builder()
        .app_context(state)
        .layer(BodyLimit::max(32 * 1024))
        .layer(app::protect)
        .layout(app::shell)
        .page(app::list)
        .route(app::providers_redirect)
        .page(app::form)
        .page(app::save)
        .page(app::perform_action)
        .route(assets::component_css)
        .route(assets::console_css)
        .route(assets::runtime_js)
        .runtime()
        .topcoat_ant_design()
        .assets(assets::config().map_err(|_| io::Error::other("无法加载控制台静态资源"))?)
        .build();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    println!("LLMProxy 控制台已启动：http://127.0.0.1:{port}");
    topcoat::serve(listener, router).await?;
    Ok(())
}
