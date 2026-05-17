use super::image::Image;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{env, path::PathBuf};
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
                let image_ref = format!("{image}");
                let mut command = Command::new("docker");
                command.args(["build", "."]);
                configure_docker_build_network(&mut command);

                let output = command
                    .args(["--tag", &image_ref])
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

fn configure_docker_build_network(command: &mut Command) {
    if let Ok(network) = env::var("DOCKER_BUILD_NETWORK") {
        let network = network.trim();
        if !network.is_empty() {
            command.args(["--network", network]);
        }
    }

    if let Ok(add_hosts) = env::var("DOCKER_BUILD_ADD_HOSTS") {
        for add_host in add_hosts
            .split(',')
            .map(str::trim)
            .filter(|add_host| !add_host.is_empty())
        {
            command.args(["--add-host", add_host]);
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
                owner: "test".to_string(),
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
                owner: "test".to_string(),
                repository: "test-build-nixpacks".to_string(),
                tag: "latest".to_string(),
            },
        );
        builder.build().await.unwrap();
    }
}
