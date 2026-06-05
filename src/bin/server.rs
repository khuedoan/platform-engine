use std::{collections::HashMap, sync::Arc};

use anyhow::{Context, Result};
use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
};
use platform_engine::{
    activities::ForgejoCommitStatusTarget, core::app::source::Source, temporal, workflows,
};
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
    repo_mappings: HashMap<String, AppTarget>,
    forgejo_url: Option<String>,
    temporal_web_url: Option<String>,
    temporal_namespace: String,
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
            repo_mappings: std::env::var("NETAMOS_WEBHOOK_REPOS")
                .ok()
                .map(|spec| parse_repo_mappings(&spec))
                .transpose()?
                .unwrap_or_default(),
            forgejo_url: std::env::var("FORGEJO_URL").ok(),
            temporal_web_url: std::env::var("TEMPORAL_WEB_URL").ok(),
            temporal_namespace: std::env::var("TEMPORAL_NAMESPACE")
                .unwrap_or_else(|_| "default".to_string()),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AppTarget {
    tenant: String,
    project: String,
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
            let source_repo = format!("{owner}/{repo_name}");
            let target = state
                .config
                .repo_mappings
                .get(&source_repo)
                .cloned()
                .unwrap_or(AppTarget {
                    tenant: owner.clone(),
                    project: repo_name.clone(),
                });

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
            let commit_status = state.config.forgejo_url.as_ref().and_then(|forgejo_url| {
                state
                    .config
                    .temporal_web_url
                    .as_ref()
                    .map(|temporal_web_url| ForgejoCommitStatusTarget {
                        forgejo_url: forgejo_url.clone(),
                        repo: source_repo.clone(),
                        sha: revision.clone(),
                        target_url: temporal_workflow_url(
                            temporal_web_url,
                            &state.config.temporal_namespace,
                            &workflow_id,
                        ),
                    })
            });

            let push_to_deploy_input =
                platform_engine::workflows::push_to_deploy::PushToDeployInput {
                    source,
                    gitops_url: state.config.gitops_url.clone(),
                    gitops_revision: state.config.gitops_revision.clone(),
                    tenant: target.tenant,
                    project: target.project,
                    environment,
                    registry: state.config.registry.clone(),
                    commit_status,
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

fn parse_repo_mappings(spec: &str) -> Result<HashMap<String, AppTarget>> {
    let mut mappings = HashMap::new();
    for entry in spec
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
    {
        let (source, target) = entry.split_once('=').unwrap_or((entry, entry));
        let source = source.trim().to_string();
        let (tenant, project) = split_repo_path(target.trim())?;
        mappings.insert(
            source,
            AppTarget {
                tenant: tenant.to_string(),
                project: project.to_string(),
            },
        );
    }

    Ok(mappings)
}

fn split_repo_path(path: &str) -> Result<(&str, &str)> {
    path.split_once('/')
        .filter(|(owner, name)| !owner.is_empty() && !name.is_empty())
        .context("repo mapping must use owner/name or owner/name=tenant/project")
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

fn temporal_workflow_url(base_url: &str, namespace: &str, workflow_id: &str) -> String {
    format!(
        "{}/namespaces/{}/workflows/{}",
        base_url.trim_end_matches('/'),
        namespace,
        workflow_id
    )
}

#[cfg(test)]
mod tests {
    use super::{AppTarget, app_environment, parse_repo_mappings, temporal_workflow_url};

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

    #[test]
    fn repo_mappings_default_to_source_repo() {
        let mappings = parse_repo_mappings("khuedoan/blog").unwrap();
        assert_eq!(
            mappings.get("khuedoan/blog"),
            Some(&AppTarget {
                tenant: "khuedoan".to_string(),
                project: "blog".to_string(),
            })
        );
    }

    #[test]
    fn repo_mappings_can_target_a_different_app() {
        let mappings = parse_repo_mappings("khuedoan/example-service=test/example").unwrap();
        assert_eq!(
            mappings.get("khuedoan/example-service"),
            Some(&AppTarget {
                tenant: "test".to_string(),
                project: "example".to_string(),
            })
        );
    }

    #[test]
    fn temporal_workflow_url_trims_base_url() {
        assert_eq!(
            temporal_workflow_url(
                "https://temporal.example.com/",
                "default",
                "push-to-deploy-example-abc123"
            ),
            "https://temporal.example.com/namespaces/default/workflows/push-to-deploy-example-abc123"
        );
    }
}
