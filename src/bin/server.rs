use std::sync::Arc;

use anyhow::{Context, Result};
use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
};
use platform_engine::{core::app::source::Source, temporal, workflows};
use serde::Deserialize;
use tokio::net::TcpListener;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

#[derive(Clone)]
struct AppState {
    client: Arc<temporalio_client::Client>,
    config: AppConfig,
}

#[derive(Clone)]
struct AppConfig {
    gitops_url: String,
    gitops_revision: String,
    registry: String,
}

impl AppConfig {
    fn from_env() -> Result<Self> {
        let gitops_base_url = std::env::var("GITOPS_URL").context("GITOPS_URL is required")?;
        let gitops_url = if gitops_base_url.ends_with(".git") {
            gitops_base_url
        } else {
            format!("{gitops_base_url}.git")
        };

        Ok(Self {
            gitops_url,
            gitops_revision: std::env::var("GITOPS_REVISION")
                .unwrap_or_else(|_| "master".to_string()),
            registry: std::env::var("REGISTRY").unwrap_or_else(|_| "localhost:5000".to_string()),
        })
    }
}

#[derive(Deserialize)]
struct Owner {
    username: String,
}

#[derive(Deserialize)]
struct RepoInfo {
    name: String,
    owner: Owner,
    #[serde(default)]
    default_branch: String,
    #[serde(default)]
    clone_url: String,
}

#[derive(Deserialize)]
struct PushPayload {
    after: String,
    #[serde(rename = "ref")]
    git_ref: String,
    repository: RepoInfo,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::fmt()
        .with_env_filter(EnvFilter::try_from_env("LOG_LEVEL").unwrap_or(EnvFilter::new("info")))
        .without_time()
        .init();

    let client = temporal::get_client().await?;
    let state = AppState {
        client: Arc::new(client),
        config: AppConfig::from_env()?,
    };

    let app = Router::new()
        .route("/webhooks/gitea", post(handle_gitea_webhook))
        .route("/healthz", post(|| async { StatusCode::NO_CONTENT }))
        .with_state(state);

    let listener = TcpListener::bind("[::]:8080").await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn handle_gitea_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let event = headers
        .get("X-Gitea-Event")
        .or_else(|| headers.get("X-Forgejo-Event"))
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();

    match event.as_str() {
        "push" => {
            let payload: PushPayload = match serde_json::from_slice(&body) {
                Ok(p) => p,
                Err(e) => {
                    error!(error = %e, "invalid JSON payload");
                    return StatusCode::BAD_REQUEST;
                }
            };

            // Map payload to Source::Git
            let owner = payload.repository.owner.username;
            let repo_name = payload.repository.name;
            let revision = payload.after;
            let environment = app_environment(&payload.git_ref, &payload.repository.default_branch);
            let url = payload.repository.clone_url;
            let workspace_path = format!("/tmp/{}-{}", sanitize(&repo_name), &revision);

            // Example JSON
            // {
            //     "Git": {
            //         "name": "example-service",
            //         "revision": "828c31f942e8913ab2af53a2841c180586c5b7e1",
            //         "url": "https://github.com/khuedoan/example-service",
            //         "path": "/tmp/example-service/828c31f942e8913ab2af53a2841c180586c5b7e1"
            //     }
            // }
            let source = Source::Git {
                name: repo_name.clone(),
                owner: owner.clone(),
                url,
                revision: revision.clone(),
                path: std::path::PathBuf::from(workspace_path),
            };

            let workflow_id = format!(
                "push-to-deploy-{}-{}",
                sanitize(&repo_name),
                &revision[..std::cmp::min(12, revision.len())]
            );

            let push_to_deploy_input =
                platform_engine::workflows::push_to_deploy::PushToDeployInput {
                    source,
                    gitops_url: state.config.gitops_url.clone(),
                    gitops_revision: state.config.gitops_revision.clone(),
                    tenant: owner,
                    project: repo_name.clone(),
                    environment,
                    registry: state.config.registry.clone(),
                };

            // Start workflow
            let start_res =
                workflows::start_workflow(&state.client, workflow_id, push_to_deploy_input).await;
            match start_res {
                Ok(_) => {
                    info!(repo = %repo_name, rev = %revision, "push_to_deploy triggered");
                    StatusCode::ACCEPTED
                }
                Err(e) => {
                    error!(error = %e, "failed to start workflow");
                    StatusCode::INTERNAL_SERVER_ERROR
                }
            }
        }
        _ => StatusCode::NO_CONTENT, // ignore unsupported events
    }
}

fn sanitize(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
            out.push(ch);
        } else if ch.is_whitespace() || ch == '/' {
            out.push('-');
        }
    }
    out.trim_matches('-').to_lowercase()
}

fn app_environment(git_ref: &str, default_branch: &str) -> String {
    let branch = branch_name(git_ref);
    if !default_branch.is_empty() && branch == branch_name(default_branch) {
        return "production".to_string();
    }

    sanitize(branch)
}

fn branch_name(git_ref: &str) -> &str {
    git_ref.strip_prefix("refs/heads/").unwrap_or(git_ref)
}

#[cfg(test)]
mod tests {
    use super::app_environment;

    #[test]
    fn app_environment_uses_production_for_default_branch() {
        assert_eq!(app_environment("refs/heads/main", "main"), "production");
        assert_eq!(app_environment("refs/heads/master", "master"), "production");
        assert_eq!(
            app_environment("refs/heads/trunk", "refs/heads/trunk"),
            "production"
        );
    }

    #[test]
    fn app_environment_uses_branch_name_for_non_default_branch() {
        assert_eq!(app_environment("refs/heads/master", "main"), "master");
        assert_eq!(
            app_environment("refs/heads/feature/foo", "main"),
            "feature-foo"
        );
    }
}
