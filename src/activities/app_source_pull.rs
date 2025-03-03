use crate::core::app::source::Source;
use serde::{Deserialize, Serialize};
use temporal_sdk::{ActContext, ActivityError};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    pub source: Source,
}

pub fn name() -> String {
    "app_source_pull".to_string()
}

pub async fn run(_ctx: ActContext, input: Input) -> Result<Source, ActivityError> {
    Ok(input.source.pull().await?)
}
