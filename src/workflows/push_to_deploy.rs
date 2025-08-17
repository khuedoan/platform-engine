use std::time::Duration;

use crate::activities::*;
use crate::core::app::{builder::Builder, image::Image, source::Source};
use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use std::path::Path;
use temporal_sdk::{ActivityOptions, WfContext, WfExitValue, WorkflowResult};
use temporal_sdk_core_protos::{coresdk::AsJsonPayloadExt, temporal::api::common::v1::RetryPolicy};
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

pub fn name() -> String {
    "push_to_deploy".to_string()
}

pub async fn definition(ctx: WfContext) -> WorkflowResult<Image> {
    let input: PushToDeployInput = serde_json::from_slice(
        &ctx.get_args()
            .first()
            .ok_or(anyhow!("missing workflow input"))?
            .data,
    )?;
    info!("starting push to deploy: {input:?}");

    let pulled_source: Source = serde_json::from_slice(
        &ctx.activity(ActivityOptions {
            activity_type: "app_source_pull".to_string(),
            input: AppSourcePullInput {
                source: input.source,
            }
            .as_json_payload()?,
            start_to_close_timeout: Some(Duration::from_secs(30)),
            ..Default::default()
        })
        .await
        .success_payload_or_error()?
        .ok_or(anyhow!("missing payload"))?
        .data,
    )?;

    let builder: Builder = serde_json::from_slice(
        &ctx.activity(ActivityOptions {
            activity_type: "app_source_detect".to_string(),
            input: AppSourceDetectInput {
                source: pulled_source,
            }
            .as_json_payload()?,
            start_to_close_timeout: Some(Duration::from_secs(5)),
            ..Default::default()
        })
        .await
        .success_payload_or_error()?
        .ok_or(anyhow!("missing payload"))?
        .data,
    )?;

    let built_image: Image = serde_json::from_slice(
        &ctx.activity(ActivityOptions {
            activity_type: "app_build".to_string(),
            input: AppBuildInput { builder }.as_json_payload()?,
            start_to_close_timeout: Some(Duration::from_secs(600)),
            retry_policy: Some(RetryPolicy {
                maximum_attempts: 1,
                ..Default::default()
            }),
            ..Default::default()
        })
        .await
        .success_payload_or_error()?
        .ok_or(anyhow!("missing payload"))?
        .data,
    )?;

    let image: Image = serde_json::from_slice(
        &ctx.activity(ActivityOptions {
            activity_type: "image_push".to_string(),
            input: ImagePushInput { image: built_image }.as_json_payload()?,
            start_to_close_timeout: Some(Duration::from_secs(120)),
            ..Default::default()
        })
        .await
        .success_payload_or_error()?
        .ok_or(anyhow!("missing payload"))?
        .data,
    )?;

    // Clone gitops repository
    let workspace: String = serde_json::from_slice(
        &ctx.activity(ActivityOptions {
            activity_type: "clone".to_string(),
            input: CloneInput {
                url: input.gitops_url,
                revision: input.gitops_revision,
            }
            .as_json_payload()?,
            start_to_close_timeout: Some(Duration::from_secs(120)),
            ..Default::default()
        })
        .await
        .success_payload_or_error()?
        .ok_or(anyhow!("missing payload"))?
        .data,
    )?;

    let apps_dir = Path::new(&workspace).join("apps");

    // Update app version
    let changed: bool = serde_json::from_slice(
        &ctx.activity(ActivityOptions {
            activity_type: "update_app_version".to_string(),
            input: UpdateAppVersionInput {
                apps_dir: apps_dir.to_string_lossy().to_string(),
                namespace: input.namespace.clone(),
                app: input.app.clone(),
                cluster: input.cluster.clone(),
                new_images: vec![AppImageUpdate {
                    repository: format!("{}/{}/{}", image.registry, image.owner, image.repository),
                    tag: image.tag.clone(),
                }],
            }
            .as_json_payload()?,
            start_to_close_timeout: Some(Duration::from_secs(30)),
            ..Default::default()
        })
        .await
        .success_payload_or_error()?
        .ok_or(anyhow!("missing payload"))?
        .data,
    )?;

    // Skip remaining steps if no changes were made
    if changed {
        // Git add changes
        let app_file_path = apps_dir
            .join(&input.namespace)
            .join(&input.app)
            .join(format!("{}.yaml", input.cluster));

        ctx.activity(ActivityOptions {
            activity_type: "git_add".to_string(),
            input: GitAddInput {
                file_path: app_file_path.to_string_lossy().to_string(),
            }
            .as_json_payload()?,
            start_to_close_timeout: Some(Duration::from_secs(30)),
            ..Default::default()
        })
        .await
        .success_payload_or_error()?;

        // Git commit changes
        let commit_message = format!(
            "chore({}/{}): update {} version",
            input.namespace, input.app, input.cluster
        );
        ctx.activity(ActivityOptions {
            activity_type: "git_commit".to_string(),
            input: GitCommitInput {
                dir: workspace.clone(),
                message: commit_message,
            }
            .as_json_payload()?,
            start_to_close_timeout: Some(Duration::from_secs(30)),
            ..Default::default()
        })
        .await
        .success_payload_or_error()?;

        ctx.activity(ActivityOptions {
            activity_type: "git_push".to_string(),
            input: GitPushInput { dir: workspace }.as_json_payload()?,
            start_to_close_timeout: Some(Duration::from_secs(60)),
            ..Default::default()
        })
        .await
        .success_payload_or_error()?;
        info!("App update completed successfully");
    } else {
        info!("No changes detected, skipping app update steps");
    }

    Ok(WfExitValue::Normal(image))
}
