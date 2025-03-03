use crate::core::app::{builder::Builder, image::Image};
use serde::{Deserialize, Serialize};
use temporal_sdk::{ActContext, ActivityError};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    pub image: Image,
}

pub fn name() -> String {
    "image_push".to_string()
}

pub async fn run(_ctx: ActContext, input: Input) -> Result<Image, ActivityError> {
    Ok(input.image.push().await?)
}
