use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    platform_engine::worker::run().await
}
