use crate::core::app::{builder::Builder, image::Image};
use serde::{Deserialize, Serialize};
use temporal_sdk::{ActContext, ActivityError};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    pub builder: Builder,
}

pub fn name() -> String {
    "app_build".to_string()
}

pub async fn run(_ctx: ActContext, input: Input) -> Result<Image, ActivityError> {
    Ok(input.builder.build().await?)
}
