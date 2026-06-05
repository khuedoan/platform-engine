use std::time::Duration;

use super::options::command_activity_options;
use crate::activities::*;
use crate::core::app::{image::Image, source::Source};
use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use temporalio_macros::{workflow, workflow_methods};
use temporalio_sdk::{WorkflowContext, WorkflowContextView, WorkflowResult};
use tracing::{info, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushToDeployInput {
    pub source: Source,
    pub gitops_url: String,
    pub gitops_revision: String,
    pub environment: String,
    pub registry: String,
    #[serde(default)]
    pub commit_status: Option<ForgejoCommitStatusTarget>,
}

#[workflow]
pub struct PushToDeployWorkflow {
    input: PushToDeployInput,
}

#[workflow_methods]
impl PushToDeployWorkflow {
    #[init]
    fn new(_ctx: &WorkflowContextView, input: PushToDeployInput) -> Self {
        Self { input }
    }

    #[run]
    pub async fn run(ctx: &mut WorkflowContext<Self>) -> WorkflowResult<Option<Image>> {
        let input = ctx.state(|state| state.input.clone());
        if !ctx.is_replaying() {
            info!("starting push to deploy: {input:?}");
        }

        let (source_owner, source_repo_name) = match git_source_repo(&input.source) {
            Some(source_repo) => source_repo,
            None => return Err(anyhow!("push-to-deploy requires a Git source").into()),
        };
        let source_repo = format!("{source_owner}/{source_repo_name}");

        set_commit_status(
            ctx,
            input.commit_status.clone(),
            "pending",
            "Deployment workflow started",
        )
        .await;

        let targets_result = ctx
            .start_activity(
                PlatformActivities::find_gitops_app_targets,
                FindGitopsAppTargetsInput {
                    url: input.gitops_url.clone(),
                    revision: input.gitops_revision.clone(),
                    registry: input.registry.clone(),
                    source_repo: source_repo.clone(),
                    environment: input.environment.clone(),
                },
                command_activity_options(Duration::from_secs(300)),
            )
            .await;
        let targets = match targets_result {
            Ok(targets) => targets,
            Err(error) => {
                set_commit_status(
                    ctx,
                    input.commit_status.clone(),
                    "failure",
                    "GitOps app target lookup failed",
                )
                .await;
                return Err(error.into());
            }
        };
        if targets.is_empty() {
            set_commit_status(
                ctx,
                input.commit_status.clone(),
                "success",
                "No matching app environment",
            )
            .await;
            if !ctx.is_replaying() {
                info!(source_repo = %source_repo, environment = %input.environment, "no matching app environment");
            }
            return Ok(None);
        }

        let image_result = ctx
            .start_activity(
                PlatformActivities::publish_image_from_source,
                PublishImageFromSourceInput {
                    source: input.source.clone(),
                    registry: input.registry.clone(),
                    image_owner: format!("apps/{source_owner}"),
                    image_repository: source_repo_name.clone(),
                },
                command_activity_options(Duration::from_secs(1200)),
            )
            .await;
        let image = match image_result {
            Ok(image) => image,
            Err(error) => {
                set_commit_status(
                    ctx,
                    input.commit_status.clone(),
                    "failure",
                    "Image build failed",
                )
                .await;
                return Err(error.into());
            }
        };

        let update = UpdateGitopsImageInput {
            url: input.gitops_url.clone(),
            revision: input.gitops_revision.clone(),
            source_repo,
            environment: input.environment.clone(),
            image: image.clone(),
            commit_status: input.commit_status.clone(),
        };

        set_commit_status(
            ctx,
            input.commit_status.clone(),
            "pending",
            "Image built; publishing GitOps update",
        )
        .await;

        let enqueue_result = ctx
            .start_activity(
                PlatformActivities::enqueue_gitops_publish,
                EnqueueGitopsPublishInput {
                    workflow_id: gitops_publish_workflow_id(&input.gitops_revision),
                    update,
                },
                command_activity_options(Duration::from_secs(300)),
            )
            .await;
        if let Err(error) = enqueue_result {
            set_commit_status(
                ctx,
                input.commit_status.clone(),
                "failure",
                "GitOps publish failed to queue",
            )
            .await;
            return Err(error.into());
        }

        if !ctx.is_replaying() {
            info!("queued GitOps and apps OCI publish");
        }

        Ok(Some(image))
    }
}

fn git_source_repo(source: &Source) -> Option<(String, String)> {
    match source {
        Source::Git { owner, name, .. } => Some((owner.clone(), name.clone())),
        Source::Docker(_) => None,
    }
}

async fn set_commit_status(
    ctx: &mut WorkflowContext<PushToDeployWorkflow>,
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

fn gitops_publish_workflow_id(revision: &str) -> String {
    format!("gitops-publisher-{}", sanitize_workflow_id(revision))
}

fn sanitize_workflow_id(input: &str) -> String {
    input
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
                ch
            } else {
                '-'
            }
        })
        .collect()
}
