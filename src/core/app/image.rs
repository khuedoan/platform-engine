use anyhow::{Context, Result};
use core::fmt;
use serde::{Deserialize, Serialize};
use tokio::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Image {
    pub registry: String,
    pub owner: String,
    pub repository: String,
    pub tag: String,
}

impl fmt::Display for Image {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{}/{}/{}:{}",
            self.registry, self.owner, self.repository, self.tag
        )
    }
}

impl Image {
    pub async fn rename(&self) -> Result<Self> {
        todo!()
    }

    pub async fn push(&self) -> Result<Self> {
        let image_ref = format!("{self}");
        let output = Command::new("docker")
            .args(["push", &image_ref])
            .output()
            .await
            .context("failed to run docker push")?;

        if !output.status.success() {
            return Err(anyhow::anyhow!(
                "failed to push image {image_ref}\n{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            ));
        }

        Ok(self.clone())
    }
}
