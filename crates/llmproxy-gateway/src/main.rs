mod config;
mod console;
mod observability;
mod proxy;
mod snapshot;

use std::error::Error;

use pingora::{proxy::http_proxy_service, server::Server};

use crate::{proxy::Gateway, snapshot::ProviderSnapshots};

fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    dotenvy::dotenv().ok();
    let settings = config::load()?;
    let _telemetry = llmproxy_telemetry::init()?;

    let (providers, _refresh) =
        ProviderSnapshots::database(&settings.database_url, &settings.master_key)?;
    let _console_runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let console = _console_runtime.block_on(llmproxy_console::Console::connect(
        &settings.database_url,
        &settings.master_key,
        settings.listen.port(),
    ))?;

    let mut server = Server::new(None)?;
    server.bootstrap();
    let mut service = http_proxy_service(&server.configuration, Gateway::new(providers, console));
    let listen = settings.listen.to_string();
    service.add_tcp(&listen);
    observability::listening(&listen);
    server.add_service(service);
    // run_forever exits the process directly, skipping the telemetry guard.
    server.run(Default::default());
    Ok(())
}
