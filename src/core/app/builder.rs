use super::image::Image;
use anyhow::Result;

pub enum Builder {
    Dockerfile,
    Nixpacks,
    Vendor(Image),
}

impl Builder {
    pub async fn build(&self) -> Result<Image> {
        match self {
            Builder::Dockerfile => todo!(),
            Builder::Nixpacks => todo!(),
            Builder::Vendor(image) => image.tag().await,
        }
    }
}
