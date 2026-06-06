use std::time::Duration;

use super::options::command_activity_options;
use crate::{
    activities::{DeleteGitopsAppInput, DeleteGitopsAppResult, PlatformActivities},
    api::DeleteAppRequest,
};
use serde::{Deserialize, Serialize};
use temporalio_macros::{workflow, workflow_methods};
use temporalio_sdk::{WorkflowContext, WorkflowContextView, WorkflowResult};
use tracing::info;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteAppInput {
    pub gitops_url: String,
    pub gitops_revision: String,
    pub registry: String,
    pub request: DeleteAppRequest,
}

#[workflow]
pub struct DeleteAppWorkflow {
    input: DeleteAppInput,
}

#[workflow_methods]
impl DeleteAppWorkflow {
    #[init]
    fn new(_ctx: &WorkflowContextView, input: DeleteAppInput) -> Self {
        Self { input }
    }

    #[run]
    pub async fn run(ctx: &mut WorkflowContext<Self>) -> WorkflowResult<DeleteGitopsAppResult> {
        let input = ctx.state(|state| state.input.clone());
        if !ctx.is_replaying() {
            info!(app = %input.request.app_path(), "deleting app environment");
        }

        let result = ctx
            .start_activity(
                PlatformActivities::delete_gitops_app,
                DeleteGitopsAppInput {
                    url: input.gitops_url,
                    revision: input.gitops_revision,
                    registry: input.registry,
                    request: input.request,
                },
                command_activity_options(Duration::from_secs(900)),
            )
            .await?;

        Ok(result)
    }
}
