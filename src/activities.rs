use crate::core::app::{builder::Builder, image::Image, source::Source};
use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use serde_yaml::Value;
use std::fs;
use std::path::{Path, PathBuf};
use temporal_sdk::{ActContext, ActivityError};
use tokio::{fs::remove_dir_all, process::Command};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSourcePullInput {
    pub source: Source,
}

pub async fn app_source_pull(
    _ctx: ActContext,
    input: AppSourcePullInput,
) -> Result<Source, ActivityError> {
    Ok(input.source.pull().await?)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSourceDetectInput {
    pub source: Source,
}

pub async fn app_source_detect(
    _ctx: ActContext,
    input: AppSourceDetectInput,
) -> Result<Builder, ActivityError> {
    Ok(input.source.detect_builder().await?)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppBuildInput {
    pub builder: Builder,
}

pub async fn app_build(_ctx: ActContext, input: AppBuildInput) -> Result<Image, ActivityError> {
    Ok(input.builder.build().await?)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImagePushInput {
    pub image: Image,
}

pub async fn image_push(_ctx: ActContext, input: ImagePushInput) -> Result<Image, ActivityError> {
    Ok(input.image.push().await?)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloneInput {
    pub url: String,
    pub revision: String,
}

pub async fn clone(_ctx: ActContext, input: CloneInput) -> Result<String, ActivityError> {
    let sanitized_url = input.url.replace(['/', ':'], "-");
    let workspace = format!(
        "/tmp/clone-{}-{}",
        sanitized_url,
        &input.revision[..std::cmp::min(8, input.revision.len())]
    );
    let workspace_path = PathBuf::from(&workspace);

    // Remove existing workspace if it exists
    if workspace_path.exists() {
        remove_dir_all(&workspace_path)
            .await
            .map_err(|e| anyhow!(e))?;
    }

    // Create the directory
    tokio::fs::create_dir_all(&workspace_path)
        .await
        .map_err(|e| anyhow!(e))?;

    // Initialize git in the target directory
    let output = Command::new("git")
        .args(["init"])
        .current_dir(&workspace_path)
        .output()
        .await
        .map_err(|e| anyhow!(e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to init repository: {}", stderr).into());
    }

    // Set git user and email from environment variables
    let git_user = std::env::var("GIT_USER").unwrap_or_else(|_| "Platform Engine".to_string());
    let git_email =
        std::env::var("GIT_EMAIL").unwrap_or_else(|_| "platform@example.com".to_string());

    let output = Command::new("git")
        .args(["config", "user.name", &git_user])
        .current_dir(&workspace_path)
        .output()
        .await
        .map_err(|e| anyhow!(e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to set git user: {}", stderr).into());
    }

    let output = Command::new("git")
        .args(["config", "user.email", &git_email])
        .current_dir(&workspace_path)
        .output()
        .await
        .map_err(|e| anyhow!(e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to set git email: {}", stderr).into());
    }

    // Create authenticated URL with credentials from environment variables
    let git_username = std::env::var("GIT_USERNAME").unwrap_or_else(|_| "git".to_string());
    let git_password = std::env::var("GIT_PASSWORD").unwrap_or_else(|_| "password".to_string());

    // Convert URL to include credentials
    let auth_url = if input.url.starts_with("http://") || input.url.starts_with("https://") {
        let url_without_protocol = input
            .url
            .trim_start_matches("http://")
            .trim_start_matches("https://");
        format!("http://{git_username}:{git_password}@{url_without_protocol}")
    } else {
        input.url.clone()
    };

    // Add remote origin with authentication
    let output = Command::new("git")
        .args(["remote", "add", "origin", &auth_url])
        .current_dir(&workspace_path)
        .output()
        .await
        .map_err(|e| anyhow!(e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to add remote: {}", stderr).into());
    }

    // Fetch with depth 1
    let output = Command::new("git")
        .args(["fetch", "--depth", "1", "origin", &input.revision])
        .current_dir(&workspace_path)
        .output()
        .await
        .map_err(|e| anyhow!(e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to fetch: {}", stderr).into());
    }

    // Checkout the specific revision
    let output = Command::new("git")
        .args(["checkout", &input.revision])
        .current_dir(&workspace_path)
        .output()
        .await
        .map_err(|e| anyhow!(e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to checkout: {}", stderr).into());
    }

    Ok(workspace)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppImageUpdate {
    pub repository: String,
    pub tag: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateAppVersionInput {
    pub apps_dir: String,
    pub namespace: String,
    pub app: String,
    pub cluster: String,
    pub new_images: Vec<AppImageUpdate>,
}

pub async fn update_app_version(
    _ctx: ActContext,
    input: UpdateAppVersionInput,
) -> Result<bool, ActivityError> {
    Ok(update_app_version_inner(input).await?)
}

pub async fn update_app_version_inner(input: UpdateAppVersionInput) -> anyhow::Result<bool> {
    let values_path = Path::new(&input.apps_dir)
        .join(&input.namespace)
        .join(&input.app)
        .join(format!("{}.yaml", input.cluster));

    let mut doc: Value = serde_yaml::from_reader(fs::File::open(&values_path)?)?;

    let mut changed = false;
    update_image_tags_recursive(&mut doc, &input.new_images, &mut changed);

    if changed {
        let writer = fs::File::create(&values_path)?;
        let mut ser = serde_yaml::Serializer::new(writer);
        doc.serialize(&mut ser)?;
    }

    Ok(changed)
}

fn update_image_tags_recursive(
    node: &mut Value,
    new_images: &[AppImageUpdate],
    changed: &mut bool,
) {
    match node {
        Value::Mapping(map) => {
            // Detect structure: image: { repository: ..., tag: ... }
            if let Some(Value::Mapping(image_map)) = map.get_mut(Value::String("image".to_string()))
            {
                let repo_key = Value::String("repository".to_string());
                let tag_key = Value::String("tag".to_string());
                let (repo_opt, tag_opt) = (
                    image_map.get(&repo_key).cloned(),
                    image_map.get(&tag_key).cloned(),
                );
                if let (Some(Value::String(repo_str)), Some(Value::String(tag_str))) =
                    (repo_opt, tag_opt)
                {
                    for img in new_images {
                        if repo_str == img.repository && tag_str != img.tag {
                            image_map.insert(tag_key.clone(), Value::String(img.tag.clone()));
                            *changed = true;
                        }
                    }
                }
            }

            // Recurse other keys
            let keys: Vec<Value> = map.keys().cloned().collect();
            for key in keys {
                if let Some(val) = map.get_mut(&key) {
                    update_image_tags_recursive(val, new_images, changed);
                }
            }
        }
        Value::Sequence(seq) => {
            for item in seq.iter_mut() {
                update_image_tags_recursive(item, new_images, changed);
            }
        }
        _ => {}
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitAddInput {
    pub file_path: String,
}

pub async fn git_add(_ctx: ActContext, input: GitAddInput) -> Result<(), ActivityError> {
    let path = PathBuf::from(&input.file_path);
    let dir = path.parent().ok_or_else(|| anyhow!("invalid file path"))?;
    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow!("invalid file path"))?;

    let status = Command::new("git")
        .args(["-C", dir.to_str().unwrap(), "add", file_name])
        .status()
        .await
        .map_err(|e| anyhow!(e))?;

    if !status.success() {
        return Err(anyhow!("git add failed").into());
    }

    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitCommitInput {
    pub dir: String,
    pub message: String,
}

pub async fn git_commit(_ctx: ActContext, input: GitCommitInput) -> Result<(), ActivityError> {
    // Set git user and email from environment variables
    let git_user = std::env::var("GIT_USER").unwrap_or_else(|_| "Platform Engine".to_string());
    let git_email =
        std::env::var("GIT_EMAIL").unwrap_or_else(|_| "platform@example.com".to_string());

    let output = Command::new("git")
        .args(["-C", &input.dir, "config", "user.name", &git_user])
        .output()
        .await
        .map_err(|e| anyhow!(e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to set git user: {}", stderr).into());
    }

    let output = Command::new("git")
        .args(["-C", &input.dir, "config", "user.email", &git_email])
        .output()
        .await
        .map_err(|e| anyhow!(e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to set git email: {}", stderr).into());
    }

    let status = Command::new("git")
        .args(["-C", &input.dir, "commit", "-m", &input.message])
        .status()
        .await
        .map_err(|e| anyhow!(e))?;
    if !status.success() {
        return Err(anyhow!("git commit failed").into());
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitPushInput {
    pub dir: String,
}

pub async fn git_push(_ctx: ActContext, input: GitPushInput) -> Result<(), ActivityError> {
    // Update the remote URL to include authentication
    let git_username = std::env::var("GIT_USERNAME").unwrap_or_else(|_| "git".to_string());
    let git_password = std::env::var("GIT_PASSWORD").unwrap_or_else(|_| "password".to_string());

    // Get current remote URL
    let get_url_output = Command::new("git")
        .args(["-C", &input.dir, "remote", "get-url", "origin"])
        .output()
        .await
        .map_err(|e| anyhow!(e))?;

    if !get_url_output.status.success() {
        return Err(anyhow!("Failed to get remote URL").into());
    }

    let current_url = String::from_utf8_lossy(&get_url_output.stdout)
        .trim()
        .to_string();

    // Update remote URL to include credentials if not already present
    if !current_url.contains("@")
        && (current_url.starts_with("http://") || current_url.starts_with("https://"))
    {
        let url_without_protocol = current_url
            .trim_start_matches("http://")
            .trim_start_matches("https://");
        let auth_url = format!("http://{git_username}:{git_password}@{url_without_protocol}");

        let set_url_output = Command::new("git")
            .args(["-C", &input.dir, "remote", "set-url", "origin", &auth_url])
            .output()
            .await
            .map_err(|e| anyhow!(e))?;

        if !set_url_output.status.success() {
            let stderr = String::from_utf8_lossy(&set_url_output.stderr);
            return Err(anyhow!("Failed to set remote URL: {}", stderr).into());
        }
    }

    let status = Command::new("git")
        .args(["-C", &input.dir, "push"])
        .status()
        .await
        .map_err(|e| anyhow!(e))?;
    if !status.success() {
        return Err(anyhow!("git push failed").into());
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushRenderedAppInput {
    pub apps_dir: String,
    pub namespace: String,
    pub app: String,
    pub cluster: String,
    pub registry: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushResult {
    pub reference: String,
    pub digest: Option<String>,
    pub size: Option<u64>,
}

pub async fn push_rendered_app(
    _ctx: ActContext,
    input: PushRenderedAppInput,
) -> Result<PushResult, ActivityError> {
    // Create a temporary directory under /tmp
    let render_dir = format!(
        "/tmp/{}-{}-render",
        input.app.replace('/', "-"),
        input.cluster
    );
    let _ = tokio::fs::remove_dir_all(&render_dir).await;
    tokio::fs::create_dir_all(&render_dir)
        .await
        .map_err(|e| anyhow!(e))?;

    // Run helm template
    // The chart is remote OCI; values path is relative to apps_dir
    let output = Command::new("helm")
        .args([
            "template",
            "--namespace",
            &input.namespace,
            &input.app,
            "oci://ghcr.io/bjw-s-labs/helm/app-template:4.1.1",
            "--values",
            &format!(
                "{}/{}/{}/{}.yaml",
                input.namespace, input.app, input.cluster, input.cluster
            ),
        ])
        .current_dir(&input.apps_dir)
        .output()
        .await
        .map_err(|e| anyhow!(e))?;

    if !output.status.success() {
        return Err(anyhow!(
            "helm template failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }

    // Write rendered output to file
    fs::write(Path::new(&render_dir).join("rendered.yaml"), &output.stdout)
        .map_err(|e| anyhow!(e))?;

    // Push with oras
    let image_ref = format!(
        "{}/{}/{}:{}",
        input.registry, input.namespace, input.app, input.cluster
    );
    let oras_output = Command::new("oras")
        .args(["push", "--format=json", "--plain-http", &image_ref, "."])
        .current_dir(&render_dir)
        .output()
        .await
        .map_err(|e| anyhow!(e))?;

    if !oras_output.status.success() {
        return Err(anyhow!(
            "oras push failed: {}",
            String::from_utf8_lossy(&oras_output.stderr)
        )
        .into());
    }

    let result: PushResult = serde_json::from_slice(&oras_output.stdout).map_err(|e| anyhow!(e))?;

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn copy_dir_recursive(src: &Path, dst: &Path) {
        if !dst.exists() {
            fs::create_dir_all(dst).unwrap();
        }
        for entry in fs::read_dir(src).unwrap() {
            let entry = entry.unwrap();
            let file_type = entry.file_type().unwrap();
            let src_path = entry.path();
            let dst_path = dst.join(entry.file_name());
            if file_type.is_dir() {
                copy_dir_recursive(&src_path, &dst_path);
            } else {
                fs::copy(&src_path, &dst_path).unwrap();
            }
        }
    }

    #[tokio::test]
    async fn test_update_app_version_changes() {
        let tmp = PathBuf::from("/tmp/test-cloudlab-apps-1");
        let _ = tokio::fs::remove_dir_all(&tmp).await;
        tokio::fs::create_dir_all(&tmp).await.unwrap();
        copy_dir_recursive(Path::new("testdata/cloudlab/apps"), &tmp);

        let changed = update_app_version_inner(UpdateAppVersionInput {
            apps_dir: tmp.to_string_lossy().to_string(),
            namespace: "khuedoan".to_string(),
            app: "blog".to_string(),
            cluster: "production".to_string(),
            new_images: vec![AppImageUpdate {
                repository: "docker.io/khuedoan/blog".to_string(),
                tag: "test-tag-123".to_string(),
            }],
        })
        .await
        .unwrap();

        assert!(changed);
    }

    #[tokio::test]
    async fn test_update_app_version_no_changes() {
        let tmp = PathBuf::from("/tmp/test-cloudlab-apps-2");
        let _ = tokio::fs::remove_dir_all(&tmp).await;
        tokio::fs::create_dir_all(&tmp).await.unwrap();
        copy_dir_recursive(Path::new("testdata/cloudlab/apps"), &tmp);

        let changed = update_app_version_inner(UpdateAppVersionInput {
            apps_dir: tmp.to_string_lossy().to_string(),
            namespace: "khuedoan".to_string(),
            app: "blog".to_string(),
            cluster: "production".to_string(),
            new_images: vec![AppImageUpdate {
                repository: "docker.io/khuedoan/blog".to_string(),
                // deliberately set same tag as in repo
                tag: "6fbd90b77a81e0bcb330fddaa230feff744a7010".to_string(),
            }],
        })
        .await
        .unwrap();

        assert!(!changed);
    }
}
