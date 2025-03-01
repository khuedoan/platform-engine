use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Image {
    registry: String,
    repository: String,
    tag: String,
}

impl Image {
    pub async fn tag(&self) -> Result<Image> {
        todo!()
    }

    pub async fn push(&self) -> Result<()> {
        todo!()
    }
}
