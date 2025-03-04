use anyhow::Result;
use app_engine::{core::app::source::Source, temporal, workflows::start_workflow};
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::fmt()
        .with_env_filter(EnvFilter::try_from_env("LOG_LEVEL").unwrap_or(EnvFilter::new("info")))
        .without_time()
        .init();

    let client = temporal::get_client().await?;

    start_workflow(
        &client,
        Source::Git {
            name: "example-service".to_string(),
            url: "https://github.com/khuedoan/example-service".to_string(),
            revision: "828c31f942e8913ab2af53a2841c180586c5b7e1".to_string(),
            path: PathBuf::from("/tmp/example-service/828c31f942e8913ab2af53a2841c180586c5b7e1"),
        },
    )
    .await
}
