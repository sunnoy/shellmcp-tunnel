#[tokio::main]
async fn main() -> anyhow::Result<()> {
    shellmcp_agent::entry::run().await
}
