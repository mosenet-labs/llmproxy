mod config;
mod console;
mod history;
mod observability;
mod proxy;
mod snapshot;
mod subscriptions;
mod tool_state;
mod transform;

use std::error::Error;

use pingora::{proxy::http_proxy_service, server::Server};

use crate::{proxy::Gateway, snapshot::ProviderSnapshots};

fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    dotenvy::dotenv().ok();
    let settings = config::load()?;
    let _telemetry = llmproxy_telemetry::init()?;
    if let Some(path) = settings.database.sqlite_path()? {
        tracing::info!(component = "gateway", event_kind = "runtime", path = %path.display(), "sqlite database selected");
    }

    let (providers, _refresh, store) = ProviderSnapshots::database(&settings.database)?;
    let _console_runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let console = _console_runtime.block_on(llmproxy_console::Console::from_store(
        store.clone(),
        settings.listen,
    ))?;

    let mut server = Server::new(None)?;
    server.bootstrap();
    let mut service = http_proxy_service(
        &server.configuration,
        Gateway::new(providers, console, store),
    );
    let listen = settings.listen.to_string();
    service.add_tcp(&listen);
    observability::listening(&listen);
    server.add_service(service);
    #[cfg(unix)]
    let run_args = pingora::server::RunArgs {
        shutdown_signal: Box::new(_refresh.shutdown_signal()),
    };
    #[cfg(not(unix))]
    let run_args = Default::default();
    // run_forever exits the process directly, skipping the telemetry guard.
    server.run(run_args);
    Ok(())
}
