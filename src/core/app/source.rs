use super::image::Image;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Source {
    Git {
        name: String,
        owner: String,
        url: String,
        revision: String,
        path: PathBuf,
    },
    Docker(Image),
}
