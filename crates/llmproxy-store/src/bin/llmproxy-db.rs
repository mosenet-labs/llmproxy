use llmproxy_store::{DatabaseConfig, ProviderStore};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();
    if std::env::args().nth(1).as_deref() != Some("migrate") {
        return Err("用法：llmproxy-db migrate".into());
    }
    let database = DatabaseConfig::from_env()?;
    ProviderStore::connect(database.url(), database.master_key())
        .await?
        .migrate()
        .await?;
    println!("Provider 数据库迁移完成");
    Ok(())
}
