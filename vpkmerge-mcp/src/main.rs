use rmcp::{transport::stdio, ServiceExt};
use vpkmerge_mcp::{config::Config, server::Server};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let service = Server::new(Config::from_env()).serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
