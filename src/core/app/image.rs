use anyhow::{Context, Result};
use bollard::{Docker, image::PushImageOptions};
use core::fmt;
use futures::stream::TryStreamExt;
use serde::{Deserialize, Serialize};

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
        let docker =
            Docker::connect_with_socket_defaults().context("failed to connect to Docker socket")?;

        docker
            .push_image(
                &format!("{}/{}/{}", self.registry, self.owner, self.repository),
                Some(PushImageOptions {
                    tag: self.tag.to_string(),
                }),
                None,
            )
            .try_for_each(|_chunk| async { Ok(()) })
            .await
            .context(format!("failed to push image {self}"))?;

        Ok(self.clone())
    }
}
