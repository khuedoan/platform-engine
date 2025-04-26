use super::{builder::Builder, image::Image};
use anyhow::Result;
use git2::{FetchOptions, Oid, Repository};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::{fs::remove_dir_all, process::Command};
use tracing::warn;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Source {
    Git {
        name: String,
        url: String,
        revision: String,
        path: PathBuf,
    },
    Docker(Image),
}

impl Source {
    pub async fn pull(&self) -> Result<Self> {
        match self {
            Source::Git {
                url,
                revision,
                path,
                ..
            } => {
                if path.exists() {
                    warn!("removing existing workspace at {path:?}");
                    remove_dir_all(&path).await?;
                }

                let repo = Repository::init(path)?;
                let mut remote = repo.remote("origin", url)?;
                remote.fetch(&[revision], Some(FetchOptions::new().depth(1)), None)?;
                let object = repo.find_object(Oid::from_str(revision)?, None)?;
                repo.checkout_tree(&object, None)?;
                repo.set_head_detached(object.id())?;

                Ok(self.clone())
            }
            Source::Docker(_image) => todo!(),
        }
    }

    pub async fn detect_builder(&self) -> Result<Builder> {
        // TODO obviously
        let registry = std::env::var("REGISTRY").unwrap_or("http://localhost:5000".to_string());

        match self {
            Source::Git {
                name,
                revision,
                path,
                ..
            } => {
                if path.join("Dockerfile").exists() {
                    Ok(Builder::Dockerfile(
                        path.to_path_buf(),
                        Image {
                            registry: registry.to_string(),
                            repository: name.to_string(),
                            tag: revision.to_string(),
                        },
                    ))
                } else if Command::new("nixpacks")
                    .args(["detect", "."])
                    .current_dir(path)
                    .output()
                    .await?
                    .stdout
                    .len()
                    // TODO nixpacks has interesting stdout
                    > 1
                {
                    Ok(Builder::Nixpacks(
                        path.to_path_buf(),
                        Image {
                            registry: registry.to_string(),
                            repository: name.to_string(),
                            tag: revision.to_string(),
                        },
                    ))
                } else {
                    Err(anyhow::anyhow!("no buildable code detected"))
                }
            }
            Source::Docker(image) => Ok(Builder::Vendor(
                image.clone(),
                Image {
                    registry: registry.to_string(),
                    repository: image.repository.clone(),
                    tag: image.tag.clone(),
                },
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_pull_git() {
        let path = PathBuf::from(
            "/tmp/workspace/example-service/828c31f942e8913ab2af53a2841c180586c5b7e1",
        );
        let source = Source::Git {
            name: "".to_string(),
            url: "https://github.com/khuedoan/example-service".to_string(),
            revision: "828c31f942e8913ab2af53a2841c180586c5b7e1".to_string(),
            path: path.clone(),
        };
        source.pull().await.unwrap();

        assert!(path.join("README.md").exists());
    }

    #[tokio::test]
    async fn test_detect_builder_nixpacks() {
        let source = Source::Git {
            name: "".to_string(),
            url: "".to_string(),
            revision: "".to_string(),
            path: PathBuf::from("testdata/example-service"),
        };
        let builder = source.detect_builder().await.unwrap();

        assert!(matches!(builder, Builder::Nixpacks { .. }))
    }

    #[tokio::test]
    async fn test_detect_builder_dockerfile() {
        let source = Source::Git {
            name: "".to_string(),
            url: "".to_string(),
            revision: "".to_string(),
            path: PathBuf::from("testdata/micropaas"),
        };
        let builder = source.detect_builder().await.unwrap();

        assert!(matches!(builder, Builder::Dockerfile { .. }))
    }
}
