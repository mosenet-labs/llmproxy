use llmproxy_store::ProviderStore;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().nth(1).as_deref() != Some("migrate") {
        return Err("用法：llmproxy-db migrate".into());
    }
    let url = std::env::var("LLMPROXY_DATABASE_URL").map_err(|_| "请设置 LLMPROXY_DATABASE_URL")?;
    let master_key =
        std::env::var("LLMPROXY_MASTER_KEY").map_err(|_| "请设置 LLMPROXY_MASTER_KEY")?;
    ProviderStore::connect(&url, &master_key)
        .await?
        .migrate()
        .await?;
    println!("Provider 数据库迁移完成");
    Ok(())
}
