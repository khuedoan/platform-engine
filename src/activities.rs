use crate::core::app::{builder::Builder, image::Image, source::Source};
use serde::{Deserialize, Serialize};
use temporal_sdk::{ActContext, ActivityError};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSourcePullInput {
    pub source: Source,
}

pub async fn app_source_pull(
    _ctx: ActContext,
    input: AppSourcePullInput,
) -> Result<Source, ActivityError> {
    Ok(input.source.pull().await?)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSourceDetectInput {
    pub source: Source,
}

pub async fn app_source_detect(
    _ctx: ActContext,
    input: AppSourceDetectInput,
) -> Result<Builder, ActivityError> {
    Ok(input.source.detect_builder().await?)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppBuildInput {
    pub builder: Builder,
}

pub async fn app_build(_ctx: ActContext, input: AppBuildInput) -> Result<Image, ActivityError> {
    Ok(input.builder.build().await?)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImagePushInput {
    pub image: Image,
}

pub async fn image_push(_ctx: ActContext, input: ImagePushInput) -> Result<Image, ActivityError> {
    Ok(input.image.push().await?)
}
