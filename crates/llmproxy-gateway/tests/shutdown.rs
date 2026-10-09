#![cfg(unix)]

#[allow(dead_code)]
mod nonstream;
mod support;

use nonstream::{Database, MASTER_KEY};
use support::Gateway;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interrupt_after_concurrent_database_requests_exits_without_panicking() {
    let database = Database::new().await;
    let mut gateway = Gateway::database(&database.url, MASTER_KEY);
    let client = reqwest::Client::new();
    let mut requests = tokio::task::JoinSet::new();
    for path in ["/ui/models", "/ui/routes", "/ui/groups", "/v1/models"] {
        let client = client.clone();
        let url = format!("http://{}{path}", gateway.address);
        let key = gateway.api_key.clone();
        requests.spawn(async move {
            for _ in 0..20 {
                let response = client.get(&url).bearer_auth(&key).send().await.unwrap();
                assert!(response.status().is_success());
                response.bytes().await.unwrap();
            }
        });
    }
    while let Some(result) = requests.join_next().await {
        result.unwrap();
    }
    // Let the periodic snapshot loader use the pool populated by HTTP traffic.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    gateway.signal("-INT");
    assert!(gateway.wait_for_exit().success());
    let logs = gateway.logs();
    assert!(logs.contains("provider snapshot refresh stopped"), "{logs}");
    assert!(!logs.contains("panicked"), "{logs}");
    assert!(!logs.contains("RecvError"), "{logs}");
}
