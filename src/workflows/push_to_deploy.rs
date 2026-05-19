use std::time::Duration;

use super::options::command_activity_options;
use crate::activities::*;
use crate::core::app::{image::Image, source::Source};
use serde::{Deserialize, Serialize};
use temporalio_macros::{workflow, workflow_methods};
use temporalio_sdk::{WorkflowContext, WorkflowContextView, WorkflowResult};
use tracing::info;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushToDeployInput {
    pub source: Source,
    pub gitops_url: String,
    pub gitops_revision: String,
    pub namespace: String,
    pub app: String,
    pub cluster: String,
    pub registry: String,
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
    pub async fn run(ctx: &mut WorkflowContext<Self>) -> WorkflowResult<Image> {
        let input = ctx.state(|state| state.input.clone());
        if !ctx.is_replaying() {
            info!("starting push to deploy: {input:?}");
        }

        let image: Image = ctx
            .start_activity(
                PlatformActivities::publish_image_from_source,
                PublishImageFromSourceInput {
                    source: input.source.clone(),
                    registry: input.registry.clone(),
                },
                command_activity_options(Duration::from_secs(1200)),
            )
            .await?;

        let update: UpdateGitopsImageResult = ctx
            .start_activity(
                PlatformActivities::update_gitops_image,
                UpdateGitopsImageInput {
                    url: input.gitops_url.clone(),
                    revision: input.gitops_revision.clone(),
                    namespace: input.namespace.clone(),
                    app: input.app.clone(),
                    cluster: input.cluster.clone(),
                    image: image.clone(),
                },
                command_activity_options(Duration::from_secs(300)),
            )
            .await?;

        if update.changed {
            if !ctx.is_replaying() {
                info!(
                    commit = ?update.commit_sha,
                    "App update completed successfully"
                );
            }
        } else if !ctx.is_replaying() {
            info!("No changes detected, skipping app update steps");
        }

        Ok(image)
    }
}
