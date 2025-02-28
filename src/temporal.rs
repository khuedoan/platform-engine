use anyhow::Result;
use temporal_client::{Client, RetryClient};
use temporal_sdk::sdk_client_options;
use temporal_sdk_core::Url;
use tracing::info;

pub async fn get_client() -> Result<RetryClient<Client>> {
    info!("connecting to Temporal");
    let server_options = sdk_client_options(Url::parse("http://localhost:7233")?).build()?;
    let client = server_options.connect("default", None).await?;
    info!("connected to Temporal");

    Ok(client)
}
