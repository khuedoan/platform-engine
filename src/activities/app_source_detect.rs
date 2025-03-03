use crate::core::app::{builder::Builder, source::Source};
use serde::{Deserialize, Serialize};
use temporal_sdk::{ActContext, ActivityError};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    pub source: Source,
}

pub fn name() -> String {
    "app_source_detect".to_string()
}

pub async fn run(_ctx: ActContext, input: Input) -> Result<Builder, ActivityError> {
    Ok(input.source.detect_builder().await?)
}
