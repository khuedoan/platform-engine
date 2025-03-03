use super::image::Image;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::process::Command;
use tracing::info;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Builder {
    Dockerfile(PathBuf),
    Nixpacks(PathBuf),
    Vendor(Image),
}

impl Builder {
    pub async fn build(&self, image: Image) -> Result<Image> {
        match self {
            Builder::Dockerfile(path) => {
                info!("building container image with Dockerfile");
                let output = Command::new("docker")
                    .args(&["build", ".", "--tag", &format!("{image}")])
                    .current_dir(path)
                    .output()
                    .await?;

                if !output.status.success() {
                    return Err(anyhow::anyhow!(
                        "{}",
                        String::from_utf8_lossy(&output.stderr)
                    ));
                }

                Ok(image.clone())
            }
            Builder::Nixpacks(path) => {
                info!("building container image with Nixpacks");
                let output = Command::new("nixpacks")
                    .args(&["build", ".", "--tag", &format!("{image}")])
                    .current_dir(path)
                    .output()
                    .await?;

                if !output.status.success() {
                    return Err(anyhow::anyhow!(
                        "{}",
                        String::from_utf8_lossy(&output.stderr)
                    ));
                }

                Ok(image.clone())
            }
            Builder::Vendor(image) => image.rename().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_build_dockerfile() {
        let builder = Builder::Dockerfile(PathBuf::from("testdata/micropaas"));
        builder
            .build(Image {
                registry: "localhost".to_string(),
                repository: "test-build-docker".to_string(),
                tag: "latest".to_string(),
            })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_build_nixpacks() {
        let builder = Builder::Nixpacks(PathBuf::from("testdata/example-service"));
        builder
            .build(Image {
                registry: "localhost".to_string(),
                repository: "test-build-nixpacks".to_string(),
                tag: "latest".to_string(),
            })
            .await
            .unwrap();
    }
}
