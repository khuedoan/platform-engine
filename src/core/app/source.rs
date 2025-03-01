use std::path::Path;

use super::{builder::Builder, image::Image};
use anyhow::Result;
use git2::{FetchOptions, Oid, Repository};
use serde::{Deserialize, Serialize};
use tokio::fs::remove_dir_all;
use tracing::warn;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Source {
    Git {
        name: String,
        url: String,
        revision: String,
    },
    Docker(Image),
}

impl Source {
    pub async fn pull(&self) -> Result<()> {
        match self {
            Source::Git {
                name,
                url,
                revision,
            } => {
                let path = Path::new("/tmp/workspace").join(name).join(revision);
                if path.exists() {
                    warn!("removing existing workspace at {path:?}");
                    remove_dir_all(&path).await?;
                }

                let repo = Repository::init(&path)?;
                let mut remote = repo.remote("origin", url)?;
                remote.fetch(&[revision], Some(FetchOptions::new().depth(1)), None)?;
                let object = repo.find_object(Oid::from_str(revision)?, None)?;
                repo.checkout_tree(&object, None)?;
                repo.set_head_detached(object.id())?;

                Ok(())
            }
            Source::Docker(_image) => todo!(),
        }
    }

    pub async fn detect_builder(&self) -> Result<Builder> {
        match self {
            Source::Git {
                name,
                url,
                revision,
            } => todo!(),
            Source::Docker(image) => Ok(Builder::Vendor(image.clone())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_pull_git() {
        let source = Source::Git {
            name: "example-service".to_string(),
            url: "https://github.com/khuedoan/example-service".to_string(),
            revision: "828c31f942e8913ab2af53a2841c180586c5b7e1".to_string(),
        };
        source.pull().await.unwrap();

        assert!(Path::new(
            "/tmp/workspace/example-service/828c31f942e8913ab2af53a2841c180586c5b7e1/README.md"
        )
        .exists());
    }
}
