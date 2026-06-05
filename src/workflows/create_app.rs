use std::time::Duration;

use super::options::command_activity_options;
use crate::{
    activities::{CreateGitopsAppInput, CreateGitopsAppResult, PlatformActivities},
    api::CreateAppRequest,
};
use serde::{Deserialize, Serialize};
use temporalio_macros::{workflow, workflow_methods};
use temporalio_sdk::{WorkflowContext, WorkflowContextView, WorkflowResult};
use tracing::info;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateAppInput {
    pub gitops_url: String,
    pub gitops_revision: String,
    pub registry: String,
    pub request: CreateAppRequest,
}

#[workflow]
pub struct CreateAppWorkflow {
    input: CreateAppInput,
}

#[workflow_methods]
impl CreateAppWorkflow {
    #[init]
    fn new(_ctx: &WorkflowContextView, input: CreateAppInput) -> Self {
        Self { input }
    }

    #[run]
    pub async fn run(ctx: &mut WorkflowContext<Self>) -> WorkflowResult<CreateGitopsAppResult> {
        let input = ctx.state(|state| state.input.clone());
        if !ctx.is_replaying() {
            info!(app = %input.request.app_path(), "creating app environment");
        }

        let result = ctx
            .start_activity(
                PlatformActivities::create_gitops_app,
                CreateGitopsAppInput {
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
