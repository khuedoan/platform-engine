use anyhow::Result;
use temporalio_client::{Client, ClientOptions, Connection, ConnectionOptions};
use temporalio_sdk_core::Url;
use tracing::info;

pub async fn get_client() -> Result<Client> {
    info!("connecting to Temporal");
    let temporal_url = std::env::var("TEMPORAL_URL").unwrap_or("http://localhost:7233".to_string());
    let connection_options = ConnectionOptions::new(Url::parse(&temporal_url)?).build();
    let connection = Connection::connect(connection_options).await?;
    let client = Client::new(connection, ClientOptions::new("default").build())?;
    info!("connected to Temporal");

    Ok(client)
}
