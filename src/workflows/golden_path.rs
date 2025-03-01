use std::time::Duration;

use crate::{activities, core::app::source::Source};
use temporal_sdk::{ActivityOptions, WfContext, WfExitValue, WorkflowResult};
use temporal_sdk_core_protos::coresdk::AsJsonPayloadExt;
use tracing::info;

pub fn name() -> String {
    "golden_path".to_string()
}

pub async fn run(ctx: WfContext) -> WorkflowResult<String> {
    info!("starting golden path");
    let _act_handle = ctx
        .activity(ActivityOptions {
            activity_type: activities::app_source_pull::name(),
            input: Source::Git {
                name: "example-service".to_string(),
                url: "https://github.com/khuedoan/example-service".to_string(),
                revision: "828c31f942e8913ab2af53a2841c180586c5b7e1".to_string(),
            }
            .as_json_payload()?,
            start_to_close_timeout: Some(Duration::from_secs(120)),
            ..Default::default()
        })
        .await
        .success_payload_or_error()?;

    Ok(WfExitValue::Normal("todo result here".to_string()))
}
