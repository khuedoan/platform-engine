use crate::workflows::{
    forgejo_bootstrap::ForgejoBootstrapInput, push_to_deploy::PushToDeployInput,
};
use anyhow::Result;
use temporalio_client::{Client, WorkflowStartOptions, errors::WorkflowStartError};
use tracing::{error, info, warn};

pub mod forgejo_bootstrap;
pub mod push_to_deploy;

pub async fn start_workflow(client: &Client, id: String, input: PushToDeployInput) -> Result<()> {
    let result = client
        .start_workflow(
            push_to_deploy::PushToDeployWorkflow::run,
            input,
            WorkflowStartOptions::new("main", id).build(),
        )
        .await;

    handle_start_result(result.map(|_| ()))
}

pub async fn start_forgejo_bootstrap(
    client: &Client,
    id: String,
    task_queue: String,
    input: ForgejoBootstrapInput,
) -> Result<()> {
    let result = client
        .start_workflow(
            forgejo_bootstrap::ForgejoBootstrapWorkflow::run,
            input,
            WorkflowStartOptions::new(task_queue, id).build(),
        )
        .await;

    handle_start_result(result.map(|_| ()))
}

fn handle_start_result(result: std::result::Result<(), WorkflowStartError>) -> Result<()> {
    match result {
        Ok(()) => {
            info!("workflow started");
            Ok(())
        }
        Err(WorkflowStartError::AlreadyStarted { .. }) => {
            warn!("workflow already exists");
            Ok(())
        }
        Err(err) => {
            error!(error = %err, "failed to start workflow");
            Err(err.into())
        }
    }
}
