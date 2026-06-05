use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fs, path::PathBuf};
use tokio::process::Command;
use tracing::{debug, info};

pub struct GitOps {
    repo_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Namespace {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct App {
    deployments: HashMap<String, Deployment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Deployment {
    replicas: i32,
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

    pub async fn read_app(
        &self,
        tenant: String,
        project: String,
        environment: String,
    ) -> Result<App> {
        let app_dir = self
            .repo_path
            .join("apps")
            .join(tenant)
            .join(project)
            .join(environment);
        debug!("reading app manifests at {app_dir:?}");

        let mut deployments = HashMap::new();
        for entry in fs::read_dir(app_dir)? {
            let path = entry?.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("yaml") {
                continue;
            }

            let manifest: Manifest = serde_yaml::from_reader(fs::File::open(&path)?)?;
            if manifest.kind == "Deployment" {
                deployments.insert(
                    manifest.metadata.name,
                    Deployment {
                        replicas: manifest.spec.and_then(|spec| spec.replicas).unwrap_or(1),
                    },
                );
            }
        }

        Ok(App { deployments })
    }

    pub async fn update_app(&self) -> Result<App> {
        todo!()
    }

    pub async fn delete_app(&self, _namespace: Namespace, _name: String) -> Result<App> {
        todo!()
    }
}

#[derive(Debug, Deserialize)]
struct Manifest {
    kind: String,
    metadata: Metadata,
    spec: Option<DeploymentSpec>,
}

#[derive(Debug, Deserialize)]
struct Metadata {
    name: String,
}

#[derive(Debug, Deserialize)]
struct DeploymentSpec {
    replicas: Option<i32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[tokio::test]
    async fn test_read_app() {
        let repo_path = PathBuf::from("/tmp/gitops");
        let _ = fs::remove_dir_all(&repo_path);
        fs::create_dir_all(repo_path.join("apps/khuedoan/blog/production")).unwrap();
        Command::new("git")
            .arg("init")
            .current_dir(&repo_path)
            .output()
            .await
            .unwrap();
        fs::write(
            repo_path.join("apps/khuedoan/blog/production/deployment-blog.yaml"),
            r#"apiVersion: apps/v1
kind: Deployment
metadata:
  name: blog
spec:
  replicas: 2
"#,
        )
        .unwrap();

        let gitops = GitOps::new("unused".to_string()).await.unwrap();

        let app = gitops
            .read_app(
                "khuedoan".to_string(),
                "blog".to_string(),
                "production".to_string(),
            )
            .await
            .unwrap();

        assert_eq!(app.deployments["blog"].replicas, 2);
    }
}
