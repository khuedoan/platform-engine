use crate::core::app::source::Source;
use anyhow::{anyhow, Result};
use temporal_client::{tonic::Code, Client, RetryClient, WorkflowOptions};
use temporal_sdk_core::WorkflowClientTrait;
use temporal_sdk_core_protos::coresdk::AsJsonPayloadExt;
use tracing::{error, info, warn};

pub mod golden_path;

pub async fn start_workflow(
    client: &RetryClient<Client>,
    id: String,
    source: Source,
) -> Result<()> {
    let input = vec![source.as_json_payload()?];

    match client
        .start_workflow(
            input,
            "main".to_string(),
            id,
            golden_path::name(),
            None,
            WorkflowOptions {
                ..Default::default()
            },
        )
        .await
    {
        Ok(response) => {
            info!("workflow started: {response:?}");
            Ok(())
        }
        Err(e) => match e.code() {
            Code::AlreadyExists => {
                warn!("workflow already exists");
                Ok(())
            }
            _ => {
                error!("failed to start workflow: {}", e.message());
                Err(anyhow!("{}", e.code()))
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::temporal;

    #[tokio::test]
    async fn test_start_workflow() {
        let client = temporal::get_client().await.unwrap();

        start_workflow(
            &client,
            "test".to_string(),
            Source::Git {
                name: "example-service".to_string(),
                url: "https://github.com/khuedoan/example-service".to_string(),
                revision: "828c31f942e8913ab2af53a2841c180586c5b7e1".to_string(),
                path: PathBuf::from(
                    "/tmp/example-service/828c31f942e8913ab2af53a2841c180586c5b7e1",
                ),
            },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_start_multiple_workflows() {
        let client = temporal::get_client().await.unwrap();

        start_workflow(
            &client,
            "test1".to_string(),
            Source::Git {
                name: "example-service".to_string(),
                url: "https://github.com/khuedoan/example-service".to_string(),
                revision: "828c31f942e8913ab2af53a2841c180586c5b7e1".to_string(),
                path: PathBuf::from(
                    "/tmp/example-service/828c31f942e8913ab2af53a2841c180586c5b7e1",
                ),
            },
        )
        .await
        .unwrap();

        start_workflow(
            &client,
            "test1".to_string(),
            Source::Git {
                name: "example-service".to_string(),
                url: "https://github.com/khuedoan/example-service".to_string(),
                revision: "828c31f942e8913ab2af53a2841c180586c5b7e1".to_string(),
                path: PathBuf::from(
                    "/tmp/example-service/828c31f942e8913ab2af53a2841c180586c5b7e1",
                ),
            },
        )
        .await
        .unwrap();

        start_workflow(
            &client,
            "test2".to_string(),
            Source::Git {
                name: "example-service".to_string(),
                url: "https://github.com/khuedoan/example-service".to_string(),
                revision: "828c31f942e8913ab2af53a2841c180586c5b7e1".to_string(),
                path: PathBuf::from(
                    "/tmp/example-service/828c31f942e8913ab2af53a2841c180586c5b7e1",
                ),
            },
        )
        .await
        .unwrap();
    }
}
