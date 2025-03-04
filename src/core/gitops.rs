use anyhow::{anyhow, Result};
use git2::Repository;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fs::File, path::PathBuf};
use tracing::{debug, info};

pub struct GitOps {
    repo: Repository,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Namespace {}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Values {
    app_template: App,
}

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

        let repo = if path.exists() {
            info!("opening existing repository at {path:?}");
            Repository::open(path)?
        } else {
            info!("cloning {url} repository to {path:?}");
            Repository::clone(&url, &path)?
        };

        Ok(Self { repo })
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
            .repo
            .workdir()
            .ok_or(anyhow!("missing workdir"))?
            .join("apps")
            .join("blog")
            .join("values.yaml");
        debug!("reading value file at {path:?}");
        let values: Values = serde_yaml::from_reader(File::open(path)?)?;

        Ok(values.app_template)
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
        GitOps::new("https://github.com/khuedoan/horus".to_string())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_read_app() {
        let gitops = GitOps::new("https://github.com/khuedoan/horus".to_string())
            .await
            .unwrap();

        let app = gitops
            .read_app("blog".to_string(), "blog".to_string())
            .await
            .unwrap();

        assert_eq!(app.controllers["main"].replicas, 2);
    }
}
