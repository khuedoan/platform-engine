use anyhow::Result;
use core::fmt;
use serde::{Deserialize, Serialize};

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
    pub async fn tag(&self) -> Result<Image> {
        todo!()
    }

    pub async fn push(&self) -> Result<()> {
        todo!()
    }
}
