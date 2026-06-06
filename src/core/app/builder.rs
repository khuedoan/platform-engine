use super::image::Image;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Builder {
    Dockerfile(PathBuf, Image),
    Nixpacks(PathBuf, Image),
    Vendor(Image, Image),
}
