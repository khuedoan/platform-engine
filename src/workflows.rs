use crate::workflows::{
    forgejo_bootstrap::ForgejoBootstrapInput, push_to_deploy::PushToDeployInput,
};
use anyhow::{Result, anyhow};
use temporal_client::{Client, RetryClient, WorkflowOptions, tonic::Code};
use temporal_sdk_core::WorkflowClientTrait;
use temporal_sdk_core_protos::coresdk::AsJsonPayloadExt;
use tracing::{error, info, warn};

pub mod forgejo_bootstrap;
pub mod push_to_deploy;

pub async fn start_workflow(
    client: &RetryClient<Client>,
    id: String,
    input: PushToDeployInput,
) -> Result<()> {
    start_workflow_with_payload(
        client,
        id,
        "main".to_string(),
        push_to_deploy::name(),
        vec![input.as_json_payload()?],
    )
    .await
}

pub async fn start_forgejo_bootstrap(
    client: &RetryClient<Client>,
    id: String,
    task_queue: String,
    input: ForgejoBootstrapInput,
) -> Result<()> {
    start_workflow_with_payload(
        client,
        id,
        task_queue,
        forgejo_bootstrap::name(),
        vec![input.as_json_payload()?],
    )
    .await
}

async fn start_workflow_with_payload(
    client: &RetryClient<Client>,
    id: String,
    task_queue: String,
    workflow_type: String,
    input_payload: Vec<temporal_sdk_core_protos::temporal::api::common::v1::Payload>,
) -> Result<()> {
    match client
        .start_workflow(
            input_payload,
            task_queue,
            id,
            workflow_type,
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
