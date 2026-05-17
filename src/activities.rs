use crate::core::app::{builder::Builder, image::Image, source::Source};
use anyhow::{Context, anyhow};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use serde_yaml::Value as YamlValue;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use temporal_sdk::{ActContext, ActivityError};
use tokio::time::{Duration, sleep};
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

    tokio::fs::create_dir_all(&workspace_path)
        .await
        .map_err(|e| anyhow!(e))?;

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

    let output = Command::new("git")
        .args(["clone", "--branch", &input.revision, &auth_url, "."])
        .current_dir(&workspace_path)
        .output()
        .await
        .map_err(|e| anyhow!(e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to clone repository: {}", stderr).into());
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

    let mut doc: YamlValue = serde_yaml::from_reader(fs::File::open(&values_path)?)?;

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
    node: &mut YamlValue,
    new_images: &[AppImageUpdate],
    changed: &mut bool,
) {
    match node {
        YamlValue::Mapping(map) => {
            // Detect structure: image: { repository: ..., tag: ... }
            if let Some(YamlValue::Mapping(image_map)) =
                map.get_mut(YamlValue::String("image".to_string()))
            {
                let repo_key = YamlValue::String("repository".to_string());
                let tag_key = YamlValue::String("tag".to_string());
                let (repo_opt, tag_opt) = (
                    image_map.get(&repo_key).cloned(),
                    image_map.get(&tag_key).cloned(),
                );
                if let (Some(YamlValue::String(repo_str)), Some(YamlValue::String(tag_str))) =
                    (repo_opt, tag_opt)
                {
                    for img in new_images {
                        if repo_str == img.repository && tag_str != img.tag {
                            image_map.insert(tag_key.clone(), YamlValue::String(img.tag.clone()));
                            *changed = true;
                        }
                    }
                }
            }

            // Recurse other keys
            let keys: Vec<YamlValue> = map.keys().cloned().collect();
            for key in keys {
                if let Some(val) = map.get_mut(&key) {
                    update_image_tags_recursive(val, new_images, changed);
                }
            }
        }
        YamlValue::Sequence(seq) => {
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
pub struct ForgejoEnsureUserInput {
    pub forgejo_url: String,
    pub username: String,
    pub email: String,
}

pub async fn forgejo_wait(_ctx: ActContext, forgejo_url: String) -> Result<(), ActivityError> {
    for _ in 0..60 {
        match forgejo_request(Method::GET, &forgejo_url, "/api/healthz", None).await {
            Ok((StatusCode::OK, _)) => return Ok(()),
            Ok(_) | Err(_) => sleep(Duration::from_secs(5)).await,
        }
    }

    Err(anyhow!("timed out waiting for Forgejo").into())
}

pub async fn forgejo_ensure_user(
    _ctx: ActContext,
    input: ForgejoEnsureUserInput,
) -> Result<(), ActivityError> {
    let path = format!("/api/v1/users/{}", input.username);
    let (status, body) = forgejo_request(Method::GET, &input.forgejo_url, &path, None)
        .await
        .map_err(|e| anyhow!(e))?;

    match status {
        StatusCode::OK => {}
        StatusCode::NOT_FOUND => {
            let password = env::var("NETAMOS_PASSWORD").context("NETAMOS_PASSWORD is required")?;
            expect_forgejo_status(
                Method::POST,
                &input.forgejo_url,
                "/api/v1/admin/users",
                Some(json!({
                    "email": input.email,
                    "username": input.username,
                    "password": password,
                    "must_change_password": false,
                    "restricted": false,
                })),
                &[StatusCode::CREATED],
            )
            .await?;
        }
        _ => return Err(forgejo_status_error(Method::GET, &path, status, &body).into()),
    }

    let password = env::var("NETAMOS_PASSWORD").context("NETAMOS_PASSWORD is required")?;
    let path = format!("/api/v1/admin/users/{}", input.username);
    expect_forgejo_status(
        Method::PATCH,
        &input.forgejo_url,
        &path,
        Some(json!({
            "password": password,
            "must_change_password": false,
            "restricted": false,
        })),
        &[StatusCode::OK],
    )
    .await?;

    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgejoEnsureRepoInput {
    pub forgejo_url: String,
    pub repo: String,
    pub private: bool,
}

pub async fn forgejo_ensure_repo(
    _ctx: ActContext,
    input: ForgejoEnsureRepoInput,
) -> Result<(), ActivityError> {
    ensure_forgejo_repo(&input.forgejo_url, &input.repo, input.private).await?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgejoEnsureWebhookInput {
    pub forgejo_url: String,
    pub repo: String,
    pub webhook_url: String,
    pub legacy_webhook_url: String,
}

pub async fn forgejo_ensure_webhook(
    _ctx: ActContext,
    input: ForgejoEnsureWebhookInput,
) -> Result<(), ActivityError> {
    let path = format!("/api/v1/repos/{}/hooks", input.repo);
    let hooks = expect_forgejo_json(Method::GET, &input.forgejo_url, &path, None).await?;
    let hooks = hooks
        .as_array()
        .ok_or_else(|| anyhow!("Forgejo hooks response is not an array"))?;

    let mut has_webhook = false;
    for hook in hooks {
        let url = hook
            .get("config")
            .and_then(|config| config.get("url"))
            .or_else(|| hook.get("url"))
            .and_then(JsonValue::as_str);

        if url == Some(input.webhook_url.as_str()) {
            has_webhook = true;
        }

        if url == Some(input.legacy_webhook_url.as_str()) {
            let hook_id = hook
                .get("id")
                .and_then(JsonValue::as_u64)
                .ok_or_else(|| anyhow!("Forgejo hook is missing id"))?;
            let path = format!("/api/v1/repos/{}/hooks/{hook_id}", input.repo);
            expect_forgejo_status(
                Method::DELETE,
                &input.forgejo_url,
                &path,
                None,
                &[StatusCode::NO_CONTENT],
            )
            .await?;
        }
    }

    if has_webhook {
        return Ok(());
    }

    expect_forgejo_status(
        Method::POST,
        &input.forgejo_url,
        &path,
        Some(json!({
            "type": "gitea",
            "config": {
                "url": input.webhook_url,
                "content_type": "json",
            },
            "events": ["push"],
            "active": true,
        })),
        &[StatusCode::CREATED],
    )
    .await?;

    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgejoEnsureCollaboratorInput {
    pub forgejo_url: String,
    pub repo: String,
    pub username: String,
    pub permission: String,
}

pub async fn forgejo_ensure_collaborator(
    _ctx: ActContext,
    input: ForgejoEnsureCollaboratorInput,
) -> Result<(), ActivityError> {
    let path = format!(
        "/api/v1/repos/{}/collaborators/{}",
        input.repo, input.username
    );
    expect_forgejo_status(
        Method::PUT,
        &input.forgejo_url,
        &path,
        Some(json!({ "permission": input.permission })),
        &[StatusCode::NO_CONTENT],
    )
    .await?;

    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgejoEnsureGitopsRepoSeededInput {
    pub forgejo_url: String,
    pub repo: String,
    pub source_url: String,
    pub revision: String,
}

pub async fn forgejo_ensure_gitops_repo_seeded(
    _ctx: ActContext,
    input: ForgejoEnsureGitopsRepoSeededInput,
) -> Result<(), ActivityError> {
    ensure_forgejo_repo(&input.forgejo_url, &input.repo, false).await?;

    let target_url = format!(
        "{}/{}.git",
        input.forgejo_url.trim_end_matches('/'),
        input.repo
    );
    let output = authenticated_git_command()
        .args(["ls-remote", &target_url, "HEAD"])
        .output()
        .await
        .map_err(|e| anyhow!(e))?;

    if !output.status.success() {
        return Err(anyhow!(
            "failed to inspect GitOps repo: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }

    if !String::from_utf8_lossy(&output.stdout).trim().is_empty() {
        return Ok(());
    }

    let workspace = PathBuf::from(format!(
        "/tmp/gitops-seed-{}",
        input.repo.replace(['/', ':'], "-")
    ));
    if workspace.exists() {
        remove_dir_all(&workspace).await.map_err(|e| anyhow!(e))?;
    }

    let output = Command::new("git")
        .args(["clone", "--branch", &input.revision, &input.source_url])
        .arg(&workspace)
        .output()
        .await
        .map_err(|e| anyhow!(e))?;

    if !output.status.success() {
        return Err(anyhow!(
            "failed to clone GitOps source repo: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }

    let output = authenticated_git_command()
        .args([
            "push",
            &target_url,
            &format!("HEAD:refs/heads/{}", input.revision),
        ])
        .current_dir(&workspace)
        .output()
        .await
        .map_err(|e| anyhow!(e))?;

    if !output.status.success() {
        return Err(anyhow!(
            "failed to seed GitOps repo: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }

    Ok(())
}

async fn ensure_forgejo_repo(forgejo_url: &str, repo: &str, private: bool) -> anyhow::Result<()> {
    let (owner, name) = split_repo(repo)?;
    let path = format!("/api/v1/repos/{owner}/{name}");
    let (status, body) = forgejo_request(Method::GET, forgejo_url, &path, None).await?;

    match status {
        StatusCode::OK => Ok(()),
        StatusCode::NOT_FOUND => {
            let path = format!("/api/v1/admin/users/{owner}/repos");
            expect_forgejo_status(
                Method::POST,
                forgejo_url,
                &path,
                Some(json!({
                    "name": name,
                    "private": private,
                })),
                &[StatusCode::CREATED, StatusCode::CONFLICT],
            )
            .await?;
            Ok(())
        }
        _ => Err(forgejo_status_error(Method::GET, &path, status, &body)),
    }
}

fn split_repo(repo: &str) -> anyhow::Result<(&str, &str)> {
    repo.split_once('/')
        .ok_or_else(|| anyhow!("invalid repo {repo}, expected owner/name"))
}

async fn expect_forgejo_json(
    method: Method,
    forgejo_url: &str,
    path: &str,
    payload: Option<JsonValue>,
) -> anyhow::Result<JsonValue> {
    let body = expect_forgejo_status(method, forgejo_url, path, payload, &[StatusCode::OK]).await?;
    Ok(serde_json::from_slice(&body)?)
}

async fn expect_forgejo_status(
    method: Method,
    forgejo_url: &str,
    path: &str,
    payload: Option<JsonValue>,
    expected: &[StatusCode],
) -> anyhow::Result<Vec<u8>> {
    let (status, body) = forgejo_request(method.clone(), forgejo_url, path, payload).await?;
    if expected.contains(&status) {
        Ok(body)
    } else {
        Err(forgejo_status_error(method, path, status, &body))
    }
}

async fn forgejo_request(
    method: Method,
    forgejo_url: &str,
    path: &str,
    payload: Option<JsonValue>,
) -> anyhow::Result<(StatusCode, Vec<u8>)> {
    let admin_username =
        env::var("FORGEJO_ADMIN_USERNAME").context("FORGEJO_ADMIN_USERNAME is required")?;
    let admin_password =
        env::var("FORGEJO_ADMIN_PASSWORD").context("FORGEJO_ADMIN_PASSWORD is required")?;
    let client = reqwest::Client::new();
    let mut request = client
        .request(
            method,
            format!("{}{}", forgejo_url.trim_end_matches('/'), path),
        )
        .basic_auth(admin_username, Some(admin_password))
        .header("Accept", "application/json");

    if let Some(payload) = payload {
        request = request.json(&payload);
    }

    let response = request.send().await?;
    let status = response.status();
    let body = response.bytes().await?.to_vec();
    Ok((status, body))
}

fn forgejo_status_error(
    method: Method,
    path: &str,
    status: StatusCode,
    body: &[u8],
) -> anyhow::Error {
    let body = String::from_utf8_lossy(body);
    anyhow!(
        "{} {} returned {}: {}",
        method.as_str(),
        path,
        status.as_u16(),
        body.chars().take(500).collect::<String>()
    )
}

fn authenticated_git_command() -> Command {
    let mut command = Command::new("git");
    command
        .env(
            "GIT_USERNAME",
            env::var("FORGEJO_ADMIN_USERNAME").unwrap_or_default(),
        )
        .env(
            "GIT_PASSWORD",
            env::var("FORGEJO_ADMIN_PASSWORD").unwrap_or_default(),
        )
        .env("GIT_TERMINAL_PROMPT", "0")
        .args([
            "-c",
            "credential.helper=!f() { echo username=$GIT_USERNAME; echo password=$GIT_PASSWORD; }; f",
        ]);
    command
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
