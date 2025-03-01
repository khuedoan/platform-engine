use std::time::Duration;

use crate::{
    activities,
    core::app::{image::Image, source::Source},
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

    let _act_handle = ctx
        .activity(ActivityOptions {
            activity_type: activities::app_source_pull::name(),
            input: source.as_json_payload()?,
            start_to_close_timeout: Some(Duration::from_secs(120)),
            ..Default::default()
        })
        .await
        .success_payload_or_error()?;

    Ok(WfExitValue::Normal(Image {
        registry: "todo".to_string(),
        repository: "todo".to_string(),
        tag: "todo".to_string(),
    }))
}
