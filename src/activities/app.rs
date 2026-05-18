use crate::core::app::{builder::Builder, image::Image, source::Source};
use serde::{Deserialize, Serialize};
use temporalio_sdk::activities::{ActivityContext, ActivityError};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSourcePullInput {
    pub source: Source,
}

pub async fn app_source_pull(
    _ctx: ActivityContext,
    input: AppSourcePullInput,
) -> Result<Source, ActivityError> {
    Ok(input.source.pull().await?)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSourceDetectInput {
    pub source: Source,
    pub registry: String,
}

pub async fn app_source_detect(
    _ctx: ActivityContext,
    input: AppSourceDetectInput,
) -> Result<Builder, ActivityError> {
    Ok(input.source.detect_builder(&input.registry).await?)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppBuildInput {
    pub builder: Builder,
}

pub async fn app_build(
    _ctx: ActivityContext,
    input: AppBuildInput,
) -> Result<Image, ActivityError> {
    Ok(input.builder.build().await?)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImagePushInput {
    pub image: Image,
}

pub async fn image_push(
    _ctx: ActivityContext,
    input: ImagePushInput,
) -> Result<Image, ActivityError> {
    Ok(input.image.push().await?)
}
