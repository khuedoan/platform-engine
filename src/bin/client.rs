use anyhow::{anyhow, Result};
use app_engine::{core::app::source::Source, temporal};
use std::path::PathBuf;
use temporal_client::{tonic::Code, WorkflowOptions};
use temporal_sdk_core::WorkflowClientTrait;
use temporal_sdk_core_protos::coresdk::AsJsonPayloadExt;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::fmt()
        .with_env_filter(EnvFilter::try_from_env("LOG_LEVEL").unwrap_or(EnvFilter::new("info")))
        .without_time()
        .init();

    let client = temporal::get_client().await?;
    let input = vec![Source::Git {
        name: "example-service".to_string(),
        url: "https://github.com/khuedoan/example-service".to_string(),
        revision: "828c31f942e8913ab2af53a2841c180586c5b7e1".to_string(),
        path: PathBuf::from("/tmp/example-service/828c31f942e8913ab2af53a2841c180586c5b7e1"),
    }
    .as_json_payload()?];

    match client
        .start_workflow(
            input,
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
