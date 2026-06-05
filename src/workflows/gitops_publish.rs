use std::time::Duration;

use super::options::command_activity_options;
use crate::activities::{PlatformActivities, UpdateGitopsImageInput, UpdateGitopsImageResult};
use temporalio_macros::{workflow, workflow_methods};
use temporalio_sdk::{SyncWorkflowContext, WorkflowContext, WorkflowContextView, WorkflowResult};
use tracing::info;

pub const PUBLISH_SIGNAL: &str = "publish";

#[workflow]
#[derive(Default)]
pub struct GitopsPublishWorkflow {
    pending: Vec<UpdateGitopsImageInput>,
}

#[workflow_methods]
impl GitopsPublishWorkflow {
    #[init]
    fn new(_ctx: &WorkflowContextView, _input: ()) -> Self {
        Self::default()
    }

    #[run]
    pub async fn run(ctx: &mut WorkflowContext<Self>) -> WorkflowResult<()> {
        loop {
            ctx.wait_condition(|state| !state.pending.is_empty()).await;
            let input = ctx.state_mut(|state| state.pending.remove(0));

            if !ctx.is_replaying() {
                info!(
                    tenant = %input.tenant,
                    project = %input.project,
                    environment = %input.environment,
                    revision = %input.revision,
                    "publishing GitOps and apps OCI update"
                );
            }

            let update: UpdateGitopsImageResult = ctx
                .start_activity(
                    PlatformActivities::update_gitops_image,
                    input,
                    command_activity_options(Duration::from_secs(900)),
                )
                .await?;

            if !ctx.is_replaying() {
                info!(
                    changed = update.changed,
                    commit = ?update.commit_sha,
                    "GitOps and apps OCI publish completed"
                );
            }

            if ctx.state(|state| state.pending.is_empty()) {
                return Ok(());
            }
        }
    }

    #[signal(name = "publish")]
    pub fn publish(&mut self, _ctx: &mut SyncWorkflowContext<Self>, input: UpdateGitopsImageInput) {
        self.pending.push(input);
    }
}
