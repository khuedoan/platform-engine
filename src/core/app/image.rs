use anyhow::Result;
use bollard::{Docker, image::PushImageOptions};
use core::fmt;
use futures::stream::StreamExt;
use serde::{Deserialize, Serialize};
use tracing::error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Image {
    pub registry: String,
    pub repository: String,
    pub tag: String,
}

impl fmt::Display for Image {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}/{}:{}", self.registry, self.repository, self.tag)
    }
}

impl Image {
    pub async fn rename(&self) -> Result<Self> {
        todo!()
    }

    pub async fn push(&self) -> Result<Self> {
        let docker = Docker::connect_with_socket_defaults()?;
        docker
            .push_image(
                &format!("{}/{}", self.registry, self.repository),
                Some(PushImageOptions {
                    tag: self.tag.to_string(),
                }),
                None,
            )
            .for_each(|result| async {
                if let Err(e) = result {
                    error!("push error: {e:?}");
                }
            })
            .await;

        Ok(self.clone())
    }
}
