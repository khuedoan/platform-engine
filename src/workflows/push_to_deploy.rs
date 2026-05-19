use std::time::Duration;

use crate::activities::*;
use crate::core::app::{builder::Builder, image::Image, source::Source};
use serde::{Deserialize, Serialize};
use std::path::Path;
use temporalio_common::protos::temporal::api::common::v1::RetryPolicy;
use temporalio_macros::{workflow, workflow_methods};
use temporalio_sdk::{ActivityOptions, WorkflowContext, WorkflowContextView, WorkflowResult};
use tracing::info;

const COMMAND_HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(30);

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

        let pulled_source: Source = ctx
            .start_activity(
                PlatformActivities::app_source_pull,
                AppSourcePullInput {
                    source: input.source.clone(),
                },
                command_activity_options(Duration::from_secs(30)),
            )
            .await?;

        let builder: Builder = ctx
            .start_activity(
                PlatformActivities::app_source_detect,
                AppSourceDetectInput {
                    source: pulled_source,
                    registry: input.registry.clone(),
                },
                ActivityOptions::start_to_close_timeout(Duration::from_secs(5)),
            )
            .await?;

        let built_image: Image = ctx
            .start_activity(
                PlatformActivities::app_build,
                AppBuildInput { builder },
                ActivityOptions::with_start_to_close_timeout(Duration::from_secs(600))
                    .heartbeat_timeout(COMMAND_HEARTBEAT_TIMEOUT)
                    .retry_policy(RetryPolicy {
                        maximum_attempts: 1,
                        ..Default::default()
                    })
                    .build(),
            )
            .await?;

        let image: Image = ctx
            .start_activity(
                PlatformActivities::image_push,
                ImagePushInput { image: built_image },
                command_activity_options(Duration::from_secs(600)),
            )
            .await?;

        let workspace: String = ctx
            .start_activity(
                PlatformActivities::clone,
                CloneInput {
                    url: input.gitops_url.clone(),
                    revision: input.gitops_revision.clone(),
                },
                command_activity_options(Duration::from_secs(120)),
            )
            .await?;

        let apps_dir = Path::new(&workspace).join("apps");

        let changed: bool = ctx
            .start_activity(
                PlatformActivities::update_app_version,
                UpdateAppVersionInput {
                    apps_dir: apps_dir.to_string_lossy().to_string(),
                    namespace: input.namespace.clone(),
                    app: input.app.clone(),
                    cluster: input.cluster.clone(),
                    new_images: vec![AppImageUpdate {
                        repository: format!(
                            "{}/{}/{}",
                            image.registry, image.owner, image.repository
                        ),
                        tag: image.tag.clone(),
                    }],
                },
                ActivityOptions::start_to_close_timeout(Duration::from_secs(30)),
            )
            .await?;

        if changed {
            let app_file_path = apps_dir
                .join(&input.namespace)
                .join(&input.app)
                .join(format!("{}.yaml", input.cluster));

            ctx.start_activity(
                PlatformActivities::git_add,
                GitAddInput {
                    file_path: app_file_path.to_string_lossy().to_string(),
                },
                ActivityOptions::start_to_close_timeout(Duration::from_secs(30)),
            )
            .await?;

            let commit_message = format!(
                "chore({}/{}): update {} version",
                input.namespace, input.app, input.cluster
            );
            ctx.start_activity(
                PlatformActivities::git_commit,
                GitCommitInput {
                    dir: workspace.clone(),
                    message: commit_message,
                },
                ActivityOptions::start_to_close_timeout(Duration::from_secs(30)),
            )
            .await?;

            ctx.start_activity(
                PlatformActivities::git_push,
                GitPushInput { dir: workspace },
                command_activity_options(Duration::from_secs(60)),
            )
            .await?;
            if !ctx.is_replaying() {
                info!("App update completed successfully");
            }
        } else {
            if !ctx.is_replaying() {
                info!("No changes detected, skipping app update steps");
            }
        }

        Ok(image)
    }
}

fn command_activity_options(timeout: Duration) -> ActivityOptions {
    ActivityOptions::with_start_to_close_timeout(timeout)
        .heartbeat_timeout(COMMAND_HEARTBEAT_TIMEOUT)
        .build()
}
