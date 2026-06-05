use std::{
    collections::BTreeMap,
    env,
    path::PathBuf,
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow};
use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
};
use platform_engine::{
    activities::{
        AppSourceTarget, AppTarget, ForgejoCommitStatusTarget, git_command_for_url,
        scan_app_source_targets,
    },
    core::app::source::Source,
    temporal, workflows,
};
use serde::Deserialize;
use tokio::{fs, net::TcpListener, process::Command, sync::Mutex};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

#[derive(Clone)]
struct AppState {
    client: Arc<temporalio_client::Client>,
    config: AppConfig,
    gitops_index: Arc<GitopsIndex>,
}

#[derive(Clone)]
struct AppConfig {
    gitops_url: String,
    gitops_revision: String,
    gitops_repo: Option<String>,
    gitops_cache_dir: PathBuf,
    gitops_index_ttl: Duration,
    registry: String,
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
            gitops_repo: repo_slug_from_git_url(&gitops_url),
            gitops_cache_dir: env::var("NETAMOS_GITOPS_CACHE_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("/tmp/netamos/gitops-index")),
            gitops_index_ttl: duration_env("NETAMOS_GITOPS_INDEX_TTL", Duration::from_secs(300)),
            gitops_url,
            gitops_revision: std::env::var("GITOPS_REVISION")
                .unwrap_or_else(|_| "master".to_string()),
            registry: std::env::var("REGISTRY").unwrap_or_else(|_| "localhost:5000".to_string()),
            forgejo_url: std::env::var("FORGEJO_URL").ok(),
            temporal_web_url: std::env::var("TEMPORAL_WEB_URL").ok(),
            temporal_namespace: std::env::var("TEMPORAL_NAMESPACE")
                .unwrap_or_else(|_| "default".to_string()),
        })
    }
}

struct GitopsIndex {
    config: GitopsIndexConfig,
    state: Mutex<GitopsIndexState>,
}

#[derive(Clone)]
struct GitopsIndexConfig {
    url: String,
    revision: String,
    registry: String,
    cache_dir: PathBuf,
    ttl: Duration,
}

#[derive(Default)]
struct GitopsIndexState {
    targets: BTreeMap<(String, String), Vec<AppTarget>>,
    refreshed_at: Option<Instant>,
}

impl GitopsIndex {
    fn new(config: &AppConfig) -> Self {
        Self {
            config: GitopsIndexConfig {
                url: config.gitops_url.clone(),
                revision: config.gitops_revision.clone(),
                registry: config.registry.clone(),
                cache_dir: config.gitops_cache_dir.clone(),
                ttl: config.gitops_index_ttl,
            },
            state: Mutex::new(GitopsIndexState::default()),
        }
    }

    async fn targets_for(&self, source_repo: &str, environment: &str) -> Vec<AppTarget> {
        if let Err(error) = self.refresh_if_stale().await {
            warn!(error = %error, "failed to refresh GitOps deployability cache");
        }

        let state = self.state.lock().await;
        state
            .targets
            .get(&(source_repo.to_string(), environment.to_string()))
            .cloned()
            .unwrap_or_default()
    }

    async fn refresh_if_stale(&self) -> Result<()> {
        let is_stale = {
            let state = self.state.lock().await;
            state
                .refreshed_at
                .is_none_or(|refreshed_at| refreshed_at.elapsed() >= self.config.ttl)
        };

        if is_stale {
            self.refresh_now().await?;
        }

        Ok(())
    }

    async fn refresh_now(&self) -> Result<()> {
        let mut state = self.state.lock().await;
        let targets = load_gitops_targets(&self.config).await?;
        let target_count = targets.len();
        state.targets = index_targets(targets);
        state.refreshed_at = Some(Instant::now());
        info!(target_count, "refreshed GitOps deployability cache");
        Ok(())
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
    let config = AppConfig::from_env()?;
    let gitops_index = Arc::new(GitopsIndex::new(&config));
    if let Err(error) = gitops_index.refresh_now().await {
        warn!(error = %error, "failed to warm GitOps deployability cache");
    }

    let state = AppState {
        client: Arc::new(client),
        config,
        gitops_index,
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

            if state.config.gitops_repo.as_deref() == Some(source_repo.as_str()) {
                if let Err(error) = state.gitops_index.refresh_now().await {
                    error!(error = %error, "failed to refresh GitOps deployability cache");
                    return StatusCode::INTERNAL_SERVER_ERROR;
                }

                info!(repo = %source_repo, "refreshed GitOps deployability cache after GitOps push");
                return StatusCode::NO_CONTENT;
            }

            let targets = state
                .gitops_index
                .targets_for(&source_repo, &environment)
                .await;
            if targets.is_empty() {
                info!(
                    repo = %source_repo,
                    environment = %environment,
                    "push is not deployable"
                );
                return StatusCode::NO_CONTENT;
            }

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

fn index_targets(targets: Vec<AppSourceTarget>) -> BTreeMap<(String, String), Vec<AppTarget>> {
    let mut index: BTreeMap<(String, String), Vec<AppTarget>> = BTreeMap::new();
    for mapping in targets {
        let key = (mapping.source_repo, mapping.target.environment.clone());
        index.entry(key).or_default().push(mapping.target);
    }

    index
}

async fn load_gitops_targets(config: &GitopsIndexConfig) -> Result<Vec<AppSourceTarget>> {
    sync_gitops_cache(config).await?;
    scan_app_source_targets(&config.cache_dir.join("apps"), &config.registry)
}

async fn sync_gitops_cache(config: &GitopsIndexConfig) -> Result<()> {
    if !config.cache_dir.join(".git").exists() {
        if config.cache_dir.exists() {
            fs::remove_dir_all(&config.cache_dir).await?;
        }
        if let Some(parent) = config.cache_dir.parent() {
            fs::create_dir_all(parent).await?;
        }

        let (username, password) = git_credentials();
        let mut command = git_command_for_url(&config.url, &username, &password);
        command
            .args(["clone", "--branch", &config.revision, &config.url])
            .arg(&config.cache_dir);
        return run_checked_command(&mut command, "git clone GitOps repo").await;
    }

    let (username, password) = git_credentials();
    let mut command = git_command_for_url(&config.url, &username, &password);
    command
        .args(["fetch", "--prune", "origin", &config.revision])
        .current_dir(&config.cache_dir);
    run_checked_command(&mut command, "git fetch GitOps repo").await?;

    let mut command = Command::new("git");
    command
        .args(["checkout", "-B", &config.revision, "FETCH_HEAD"])
        .current_dir(&config.cache_dir);
    run_checked_command(&mut command, "git checkout GitOps repo").await?;

    let mut command = Command::new("git");
    command
        .args(["reset", "--hard", "FETCH_HEAD"])
        .current_dir(&config.cache_dir);
    run_checked_command(&mut command, "git reset GitOps repo").await?;

    Ok(())
}

fn git_credentials() -> (String, String) {
    let username = env::var("GIT_USERNAME")
        .or_else(|_| env::var("NETAMOS_USERNAME"))
        .unwrap_or_else(|_| "git".to_string());
    let password = env::var("GIT_PASSWORD")
        .or_else(|_| env::var("NETAMOS_PASSWORD"))
        .unwrap_or_else(|_| "password".to_string());

    (username, password)
}

async fn run_checked_command(command: &mut Command, operation: &str) -> Result<()> {
    command
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = command
        .output()
        .await
        .with_context(|| format!("failed to start {operation}"))?;
    if output.status.success() {
        return Ok(());
    }

    Err(anyhow!(
        "{operation} failed\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    ))
}

fn repo_slug_from_git_url(url: &str) -> Option<String> {
    let url = url.trim_end_matches(".git");
    let path = if let Some((_, rest)) = url.split_once("://") {
        rest.split_once('/').map(|(_, path)| path)?
    } else if let Some((_, path)) = url.split_once(':') {
        path
    } else {
        url
    };

    let mut parts = path.trim_matches('/').rsplitn(2, '/');
    let repo = parts.next()?.trim();
    let owner = parts.next()?.trim();
    if owner.is_empty() || repo.is_empty() {
        return None;
    }

    Some(format!("{owner}/{repo}"))
}

fn duration_env(name: &str, default: Duration) -> Duration {
    env::var(name)
        .ok()
        .and_then(|value| parse_duration(&value))
        .unwrap_or(default)
}

fn parse_duration(value: &str) -> Option<Duration> {
    let value = value.trim();
    let (number, multiplier) = value
        .strip_suffix('h')
        .map(|number| (number, 3600))
        .or_else(|| value.strip_suffix('m').map(|number| (number, 60)))
        .or_else(|| value.strip_suffix('s').map(|number| (number, 1)))
        .unwrap_or((value, 1));

    number
        .parse::<u64>()
        .ok()
        .map(|seconds| Duration::from_secs(seconds * multiplier))
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
    use std::time::Duration;

    use platform_engine::activities::{AppSourceTarget, AppTarget};

    use super::{
        app_environment, index_targets, parse_duration, repo_slug_from_git_url,
        temporal_workflow_url,
    };

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

    #[test]
    fn repo_slug_from_git_url_extracts_owner_and_repo() {
        assert_eq!(
            repo_slug_from_git_url("http://forgejo/khuedoan/cloudlab.git"),
            Some("khuedoan/cloudlab".to_string())
        );
        assert_eq!(
            repo_slug_from_git_url("git@forgejo:khuedoan/cloudlab.git"),
            Some("khuedoan/cloudlab".to_string())
        );
        assert_eq!(repo_slug_from_git_url("cloudlab"), None);
    }

    #[test]
    fn parse_duration_accepts_common_suffixes() {
        assert_eq!(parse_duration("30"), Some(Duration::from_secs(30)));
        assert_eq!(parse_duration("5m"), Some(Duration::from_secs(300)));
        assert_eq!(parse_duration("2h"), Some(Duration::from_secs(7200)));
        assert_eq!(parse_duration("bad"), None);
    }

    #[test]
    fn index_targets_groups_by_source_repo_and_environment() {
        let index = index_targets(vec![
            AppSourceTarget {
                source_repo: "khuedoan/blog".to_string(),
                target: AppTarget {
                    tenant: "khuedoan".to_string(),
                    project: "blog".to_string(),
                    environment: "production".to_string(),
                },
            },
            AppSourceTarget {
                source_repo: "khuedoan/blog".to_string(),
                target: AppTarget {
                    tenant: "khuedoan".to_string(),
                    project: "docs".to_string(),
                    environment: "production".to_string(),
                },
            },
        ]);

        assert_eq!(
            index
                .get(&("khuedoan/blog".to_string(), "production".to_string()))
                .map(Vec::len),
            Some(2)
        );
        assert!(!index.contains_key(&("khuedoan/blog".to_string(), "staging".to_string())));
    }
}
