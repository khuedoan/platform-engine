use crate::core::app::{builder::Builder, source::Source};
use anyhow::{anyhow, Result};
use temporal_sdk::{ActContext, ActivityError};

pub fn name() -> String {
    "app_source_detect".to_string()
}

pub async fn run(_ctx: ActContext, source: Source) -> Result<Builder, ActivityError> {
    let builder = source
        .detect_builder()
        .await
        .map_err(|e| ActivityError::NonRetryable(anyhow!("failed to detect builder: {e}")))?;

    Ok(builder)
}
