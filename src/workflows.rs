use crate::workflows::push_to_deploy::PushToDeployInput;
use anyhow::{Result, anyhow};
use temporal_client::{Client, RetryClient, WorkflowOptions, tonic::Code};
use temporal_sdk_core::WorkflowClientTrait;
use temporal_sdk_core_protos::coresdk::AsJsonPayloadExt;
use tracing::{error, info, warn};

pub mod push_to_deploy;

pub async fn start_workflow(
    client: &RetryClient<Client>,
    id: String,
    input: PushToDeployInput,
) -> Result<()> {
    let input_payload = vec![input.as_json_payload()?];

    match client
        .start_workflow(
            input_payload,
            "main".to_string(),
            id,
            push_to_deploy::name(),
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
