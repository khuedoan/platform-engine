use anyhow::Result;

#[derive(Debug, Clone)]
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
