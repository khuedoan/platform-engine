use super::image::Image;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::process::Command;
use tracing::info;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Builder {
    Dockerfile(PathBuf, Image),
    Nixpacks(PathBuf, Image),
    Vendor(Image, Image),
}

impl Builder {
    pub async fn build(&self) -> Result<Image> {
        match self {
            Builder::Dockerfile(path, image) => {
                info!("building container image with Dockerfile");
                let output = Command::new("docker")
                    .args(["build", ".", "--tag", &format!("{image}")])
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
            Builder::Nixpacks(path, image) => {
                info!("building container image with Nixpacks");
                let output = Command::new("nixpacks")
                    .args(["build", ".", "--tag", &format!("{image}")])
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
            Builder::Vendor(source_image, _image) => source_image.rename().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_build_dockerfile() {
        let builder = Builder::Dockerfile(
            PathBuf::from("testdata/micropaas"),
            Image {
                registry: "localhost:5000".to_string(),
                repository: "test-build-dockerfile".to_string(),
                tag: "latest".to_string(),
            },
        );
        builder.build().await.unwrap();
    }

    #[tokio::test]
    async fn test_build_nixpacks() {
        let builder = Builder::Nixpacks(
            PathBuf::from("testdata/example-service"),
            Image {
                registry: "localhost:5000".to_string(),
                repository: "test-build-nixpacks".to_string(),
                tag: "latest".to_string(),
            },
        );
        builder.build().await.unwrap();
    }
}
