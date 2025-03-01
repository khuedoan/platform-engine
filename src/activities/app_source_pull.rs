use anyhow::{anyhow, Result};
use temporal_sdk::{ActContext, ActivityError};

use crate::core::app::source::Source;

pub fn name() -> String {
    "app_source_pull".to_string()
}

pub async fn run(_ctx: ActContext, source: Source) -> Result<String, ActivityError> {
    source
        .pull()
        .await
        .map_err(|e| ActivityError::NonRetryable(anyhow!("failed to pull source: {e}")))?;

    Ok("TODO can't be () or struct, gotta be string".to_string())
}
