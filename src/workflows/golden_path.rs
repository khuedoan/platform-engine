use std::time::Duration;

use crate::activities::*;
use crate::core::app::{builder::Builder, image::Image, source::Source};
use anyhow::anyhow;
use temporal_sdk::{ActivityOptions, WfContext, WfExitValue, WorkflowResult};
use temporal_sdk_core_protos::coresdk::AsJsonPayloadExt;
use tracing::info;

pub fn name() -> String {
    "golden_path".to_string()
}

pub async fn definition(ctx: WfContext) -> WorkflowResult<Image> {
    let source: Source = serde_json::from_slice(
        &ctx.get_args()
            .first()
            .ok_or(anyhow!("missing workflow input"))?
            .data,
    )?;
    info!("starting golden path: {source:?}");

    let pulled_source: Source = serde_json::from_slice(
        &ctx.activity(ActivityOptions {
            activity_type: "app_source_pull".to_string(),
            input: AppSourcePullInput { source }.as_json_payload()?,
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
            activity_type: "app_source_detect".to_string(),
            input: AppSourceDetectInput {
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
            activity_type: "app_build".to_string(),
            input: AppBuildInput { builder }.as_json_payload()?,
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
            activity_type: "image_push".to_string(),
            input: ImagePushInput { image }.as_json_payload()?,
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
