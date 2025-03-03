use std::time::Duration;

use crate::{
    activities::{self, app_build, app_source_detect, app_source_pull, image_push},
    core::app::{builder::Builder, image::Image, source::Source},
};
use anyhow::anyhow;
use temporal_sdk::{ActivityOptions, WfContext, WfExitValue, WorkflowResult};
use temporal_sdk_core_protos::coresdk::AsJsonPayloadExt;
use tracing::info;

pub fn name() -> String {
    "golden_path".to_string()
}

pub async fn run(ctx: WfContext) -> WorkflowResult<Image> {
    let source: Source = serde_json::from_slice(
        &ctx.get_args()
            .first()
            .ok_or(anyhow!("missing workflow input"))?
            .data,
    )?;
    info!("starting golden path: {source:?}");

    let pulled_source: Source = serde_json::from_slice(
        &ctx.activity(ActivityOptions {
            activity_type: activities::app_source_pull::name(),
            input: app_source_pull::Input { source }.as_json_payload()?,
            start_to_close_timeout: Some(Duration::from_secs(120)),
            ..Default::default()
        })
        .await
        .success_payload_or_error()?
        .ok_or(anyhow!("missing payload"))?
        .data,
    )?;

    let builder: Builder = serde_json::from_slice(
        &ctx.activity(ActivityOptions {
            activity_type: activities::app_source_detect::name(),
            input: app_source_detect::Input {
                source: pulled_source,
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

    let image: Image = serde_json::from_slice(
        &ctx.activity(ActivityOptions {
            activity_type: activities::app_build::name(),
            input: app_build::Input { builder }.as_json_payload()?,
            start_to_close_timeout: Some(Duration::from_secs(600)),
            ..Default::default()
        })
        .await
        .success_payload_or_error()?
        .ok_or(anyhow!("missing payload"))?
        .data,
    )?;

    let image: Image = serde_json::from_slice(
        &ctx.activity(ActivityOptions {
            activity_type: activities::image_push::name(),
            input: image_push::Input { image }.as_json_payload()?,
            start_to_close_timeout: Some(Duration::from_secs(600)),
            ..Default::default()
        })
        .await
        .success_payload_or_error()?
        .ok_or(anyhow!("missing payload"))?
        .data,
    )?;

    Ok(WfExitValue::Normal(image))
}
