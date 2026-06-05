use super::{git_auth::authenticated_git_command, process::run_checked_command};
use anyhow::{Context, anyhow};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use std::env;
use std::path::PathBuf;
use temporalio_sdk::activities::{ActivityContext, ActivityError};
use tokio::time::{Duration, sleep};
use tokio::{fs::remove_dir_all, process::Command};

pub const FORGEJO_COMMIT_STATUS_CONTEXT: &str = "netamos/push-to-deploy";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgejoEnsureUserInput {
    pub forgejo_url: String,
    pub username: String,
    pub email: String,
}

pub async fn forgejo_wait(ctx: ActivityContext, forgejo_url: String) -> Result<(), ActivityError> {
    for _ in 0..60 {
        if ctx.is_cancelled() {
            return Err(ActivityError::cancelled());
        }
        ctx.record_heartbeat(vec![]);

        match forgejo_request(Method::GET, &forgejo_url, "/api/healthz", None).await {
            Ok((StatusCode::OK, _)) => return Ok(()),
            Ok(_) | Err(_) => {
                tokio::select! {
                    _ = sleep(Duration::from_secs(5)) => {}
                    _ = ctx.cancelled() => return Err(ActivityError::cancelled()),
                }
            }
        }
    }

    Err(anyhow!("timed out waiting for Forgejo").into())
}

pub async fn forgejo_ensure_user(
    _ctx: ActivityContext,
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
    _ctx: ActivityContext,
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
    _ctx: ActivityContext,
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
pub struct ForgejoEnsureSystemWebhookInput {
    pub forgejo_url: String,
    pub webhook_url: String,
    pub legacy_webhook_url: String,
}

pub async fn forgejo_ensure_system_webhook(
    _ctx: ActivityContext,
    input: ForgejoEnsureSystemWebhookInput,
) -> Result<(), ActivityError> {
    let path = "/api/v1/admin/hooks?type=system";
    let hooks = expect_forgejo_json(Method::GET, &input.forgejo_url, path, None).await?;
    let hooks = hooks
        .as_array()
        .ok_or_else(|| anyhow!("Forgejo system hooks response is not an array"))?;

    let mut has_webhook = false;
    for hook in hooks {
        let url = hook_url(hook);

        if url == Some(input.webhook_url.as_str()) {
            has_webhook = true;
        }

        if url == Some(input.legacy_webhook_url.as_str()) {
            delete_admin_hook(&input.forgejo_url, hook).await?;
        }
    }

    if has_webhook {
        return Ok(());
    }

    expect_forgejo_status(
        Method::POST,
        &input.forgejo_url,
        "/api/v1/admin/hooks",
        Some(json!({
            "type": "gitea",
            "config": {
                "url": input.webhook_url,
                "content_type": "json",
                "is_system_webhook": true,
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
pub struct ForgejoDeleteWebhookInput {
    pub forgejo_url: String,
    pub repo: String,
    pub webhook_url: String,
    pub legacy_webhook_url: String,
}

pub async fn forgejo_delete_webhook(
    _ctx: ActivityContext,
    input: ForgejoDeleteWebhookInput,
) -> Result<(), ActivityError> {
    let path = format!("/api/v1/repos/{}/hooks", input.repo);
    let hooks = expect_forgejo_json(Method::GET, &input.forgejo_url, &path, None).await?;
    let hooks = hooks
        .as_array()
        .ok_or_else(|| anyhow!("Forgejo hooks response is not an array"))?;

    for hook in hooks {
        let url = hook_url(hook);
        if url == Some(input.webhook_url.as_str()) || url == Some(input.legacy_webhook_url.as_str())
        {
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

    Ok(())
}

fn hook_url(hook: &JsonValue) -> Option<&str> {
    hook.get("config")
        .and_then(|config| config.get("url"))
        .or_else(|| hook.get("url"))
        .and_then(JsonValue::as_str)
}

async fn delete_admin_hook(forgejo_url: &str, hook: &JsonValue) -> Result<(), ActivityError> {
    let hook_id = hook
        .get("id")
        .and_then(JsonValue::as_u64)
        .ok_or_else(|| anyhow!("Forgejo hook is missing id"))?;
    let path = format!("/api/v1/admin/hooks/{hook_id}");
    expect_forgejo_status(
        Method::DELETE,
        forgejo_url,
        &path,
        None,
        &[StatusCode::NO_CONTENT],
    )
    .await?;

    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgejoCommitStatusTarget {
    pub forgejo_url: String,
    pub repo: String,
    pub sha: String,
    pub target_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgejoCreateCommitStatusInput {
    pub target: ForgejoCommitStatusTarget,
    pub state: String,
    pub description: String,
}

pub async fn forgejo_create_commit_status(
    _ctx: ActivityContext,
    input: ForgejoCreateCommitStatusInput,
) -> Result<(), ActivityError> {
    let path = format!(
        "/api/v1/repos/{}/statuses/{}",
        input.target.repo, input.target.sha
    );
    expect_forgejo_status(
        Method::POST,
        &input.target.forgejo_url,
        &path,
        Some(json!({
            "context": FORGEJO_COMMIT_STATUS_CONTEXT,
            "description": input.description,
            "state": input.state,
            "target_url": input.target.target_url,
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
    _ctx: ActivityContext,
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
    ctx: ActivityContext,
    input: ForgejoEnsureGitopsRepoSeededInput,
) -> Result<(), ActivityError> {
    ensure_forgejo_repo(&input.forgejo_url, &input.repo, false).await?;

    let target_url = format!(
        "{}/{}.git",
        input.forgejo_url.trim_end_matches('/'),
        input.repo
    );
    let mut command = forgejo_git_command();
    command.args(["ls-remote", &target_url, "HEAD"]);
    let output = run_checked_command(&ctx, &mut command, "git ls-remote GitOps repo").await?;

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

    let mut command = Command::new("git");
    command
        .args(["clone", "--branch", &input.revision, &input.source_url])
        .arg(&workspace);
    run_checked_command(&ctx, &mut command, "git clone GitOps source repo").await?;

    let branch_ref = format!("HEAD:refs/heads/{}", input.revision);
    let mut command = forgejo_git_command();
    command
        .args(["push", &target_url, &branch_ref])
        .current_dir(&workspace);
    run_checked_command(&ctx, &mut command, "git push GitOps seed").await?;

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

fn forgejo_git_command() -> Command {
    authenticated_git_command(
        &env::var("FORGEJO_ADMIN_USERNAME").unwrap_or_default(),
        &env::var("FORGEJO_ADMIN_PASSWORD").unwrap_or_default(),
    )
}
