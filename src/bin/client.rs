use anyhow::{anyhow, Result};
use app_engine::temporal;
use temporal_client::{tonic::Code, WorkflowOptions};
use temporal_sdk_core::WorkflowClientTrait;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::fmt()
        .with_env_filter(EnvFilter::try_from_env("LOG_LEVEL").unwrap_or(EnvFilter::new("info")))
        .without_time()
        .init();

    let client = temporal::get_client().await?;

    match client
        .start_workflow(
            vec![],
            "main".to_string(),
            "zGtLfDcgmBqBUya1qTpzRzpBpoHx-86b1a059da167ae0a4da82e3168c789e73884f5e".to_string(),
            "golden_path".to_string(),
            None,
            WorkflowOptions {
                ..Default::default()
            },
        )
        .await
    {
        Ok(response) => info!("workflow started: {response:?}"),
        Err(e) => match e.code() {
            Code::AlreadyExists => warn!("workflow already exists"),
            _ => {
                error!("failed to start workflow: {}", e.message());
                return Err(anyhow!("{}", e.code()));
            }
        },
    }

    Ok(())
}
