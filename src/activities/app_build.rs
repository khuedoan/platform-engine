use crate::core::app::{builder::Builder, image::Image};
use anyhow::{anyhow, Result};
use temporal_sdk::{ActContext, ActivityError};

pub fn name() -> String {
    "app_build".to_string()
}

pub async fn run(_ctx: ActContext, builder: Builder) -> Result<Image, ActivityError> {
    let image = builder
        // TODO don't hard code obviously
        .build(Image {
            registry: "zot.zot.svc.cluster.local".to_string(),
            repository: "example-service".to_string(),
            tag: "828c31f942e8913ab2af53a2841c180586c5b7e1".to_string(),
        })
        .await
        .map_err(|e| ActivityError::NonRetryable(anyhow!("failed to build: {e}")))?;

    Ok(image)
}
