use std::time::Duration;

use super::options::command_activity_options;
use crate::activities::{
    ForgejoCommitStatusTarget, ForgejoCreateCommitStatusInput, PlatformActivities,
    UpdateGitopsImageInput,
};
use temporalio_macros::{workflow, workflow_methods};
use temporalio_sdk::{SyncWorkflowContext, WorkflowContext, WorkflowContextView, WorkflowResult};
use tracing::{info, warn};

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
                    source_repo = %input.source_repo,
                    environment = %input.environment,
                    revision = %input.revision,
                    "publishing GitOps and apps OCI update"
                );
            }

            let commit_status = input.commit_status.clone();
            set_commit_status(
                ctx,
                commit_status.clone(),
                "pending",
                "Publishing GitOps update",
            )
            .await;

            let update_result = ctx
                .start_activity(
                    PlatformActivities::update_gitops_image,
                    input,
                    command_activity_options(Duration::from_secs(900)),
                )
                .await;
            let update = match update_result {
                Ok(update) => update,
                Err(error) => {
                    set_commit_status(
                        ctx,
                        commit_status.clone(),
                        "failure",
                        "GitOps publish failed",
                    )
                    .await;
                    return Err(error.into());
                }
            };

            set_commit_status(
                ctx,
                commit_status.clone(),
                "success",
                "GitOps update published",
            )
            .await;

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

async fn set_commit_status(
    ctx: &mut WorkflowContext<GitopsPublishWorkflow>,
    target: Option<ForgejoCommitStatusTarget>,
    state: &str,
    description: &str,
) {
    let Some(target) = target else {
        return;
    };

    let result = ctx
        .start_activity(
            PlatformActivities::forgejo_create_commit_status,
            ForgejoCreateCommitStatusInput {
                target,
                state: state.to_string(),
                description: description.to_string(),
            },
            command_activity_options(Duration::from_secs(30)),
        )
        .await;
    if let Err(error) = result {
        if !ctx.is_replaying() {
            warn!(error = %error, "failed to create Forgejo commit status");
        }
    }
}
