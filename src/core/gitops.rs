use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fs::File, path::PathBuf};
use tokio::process::Command;
use tracing::{debug, info};

pub struct GitOps {
    repo_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Namespace {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct App {
    controllers: HashMap<String, Controller>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Controller {
    replicas: i32,
    strategy: Strategy,
    containers: HashMap<String, Container>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Container {
    image: Image,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Image {
    repository: String,
    tag: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Strategy {
    RollingUpdate,
}

impl GitOps {
    pub async fn new(url: String) -> Result<Self> {
        let path = PathBuf::from("/tmp/gitops");

        if path.exists() {
            info!("opening existing repository at {path:?}");
            // Verify it's a git repository
            let output = Command::new("git")
                .args(["rev-parse", "--git-dir"])
                .current_dir(&path)
                .output()
                .await?;

            if !output.status.success() {
                return Err(anyhow!("Directory exists but is not a git repository"));
            }
        } else {
            info!("cloning {url} repository to {path:?}");
            let output = Command::new("git")
                .args(["clone", &url, path.to_str().unwrap()])
                .output()
                .await?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(anyhow!("Failed to clone repository: {}", stderr));
            }
        }

        Ok(Self { repo_path: path })
    }

    pub async fn create_namespace(&self, _name: String) -> Result<Namespace> {
        todo!()
    }

    pub async fn get_namespace(&self, _name: String) -> Result<Namespace> {
        todo!()
    }

    pub async fn create_app(&self, _namespace: Namespace, _name: String) -> Result<App> {
        todo!()
    }

    pub async fn read_app(&self, _namespace: String, _name: String) -> Result<App> {
        let path = self
            .repo_path
            .join("apps")
            .join("khuedoan")
            .join("blog")
            .join("production.yaml");
        debug!("reading value file at {path:?}");
        let app: App = serde_yaml::from_reader(File::open(path)?)?;

        Ok(app)
    }

    pub async fn update_app(&self) -> Result<App> {
        todo!()
    }

    pub async fn delete_app(&self, _namespace: Namespace, _name: String) -> Result<App> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_new_gitops() {
        GitOps::new("https://github.com/khuedoan/cloudlab".to_string())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_read_app() {
        let gitops = GitOps::new("https://github.com/khuedoan/cloudlab".to_string())
            .await
            .unwrap();

        let app = gitops
            .read_app("blog".to_string(), "blog".to_string())
            .await
            .unwrap();

        assert_eq!(app.controllers["main"].replicas, 2);
    }
}
