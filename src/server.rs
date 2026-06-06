use std::{
    collections::BTreeMap,
    env,
    path::PathBuf,
    process::Stdio,
    str::FromStr,
    sync::Arc,
    time::{Duration, Instant},
};

use crate::{
    activities::{ForgejoCommitStatusTarget, git_command_for_url},
    api::{
        AuthConfig as ApiAuthConfig, CreateAppRequest, DeleteAppRequest, DeployRequest,
        ProjectSummary, UserInfo, WorkflowStarted, WorkflowStatus,
    },
    core::app::source::Source,
    gitops::{AppSourceTarget, AppTarget, scan_app_inventory, scan_app_source_targets},
    temporal,
    workflows::{self, push_to_deploy::PushToDeployInput},
};
use anyhow::{Context, Result, anyhow};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path as AxumPath, State},
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
    routing::{delete, get, post},
};
use openidconnect::{
    ClientId, IssuerUrl, Nonce,
    core::{CoreClient, CoreIdToken, CoreProviderMetadata},
    reqwest as oidc_reqwest,
};
use serde::Deserialize;
use serde_json::json;
use tokio::{fs, net::TcpListener, process::Command, sync::Mutex};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

#[derive(Clone)]
struct AppState {
    client: Arc<temporalio_client::Client>,
    config: AppConfig,
    gitops_index: Arc<GitopsIndex>,
    auth: Arc<AuthVerifier>,
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
    forgejo_public_url: Option<String>,
    temporal_web_url: Option<String>,
    temporal_namespace: String,
    oidc_issuer: Option<String>,
    oidc_client_id: String,
    oidc_audience: String,
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
            forgejo_public_url: std::env::var("FORGEJO_PUBLIC_URL").ok(),
            temporal_web_url: std::env::var("TEMPORAL_WEB_URL").ok(),
            temporal_namespace: std::env::var("TEMPORAL_NAMESPACE")
                .unwrap_or_else(|_| "default".to_string()),
            oidc_issuer: std::env::var("OIDC_ISSUER").ok(),
            oidc_client_id: std::env::var("OIDC_CLIENT_ID")
                .unwrap_or_else(|_| "netamos-cli".to_string()),
            oidc_audience: std::env::var("OIDC_AUDIENCE")
                .unwrap_or_else(|_| "netamos-api".to_string()),
        })
    }
}

struct AuthVerifier {
    http: oidc_reqwest::Client,
    issuer: Option<String>,
    client_id: String,
    audience: String,
}

impl AuthVerifier {
    fn new(config: &AppConfig) -> Result<Self> {
        Ok(Self {
            http: oidc_http_client()?,
            issuer: config.oidc_issuer.clone(),
            client_id: config.oidc_client_id.clone(),
            audience: config.oidc_audience.clone(),
        })
    }

    fn config(&self) -> Result<ApiAuthConfig, ApiError> {
        let issuer = self
            .issuer
            .clone()
            .ok_or_else(|| ApiError::unavailable("OIDC_ISSUER is not configured"))?;
        Ok(ApiAuthConfig {
            issuer,
            client_id: self.client_id.clone(),
            scopes: vec![
                "openid".to_string(),
                "profile".to_string(),
                "email".to_string(),
                "offline_access".to_string(),
                format!("audience:server:client_id:{}", self.audience),
            ],
        })
    }

    async fn verify(&self, headers: &HeaderMap) -> Result<UserInfo, ApiError> {
        let issuer = self
            .issuer
            .as_deref()
            .ok_or_else(|| ApiError::unavailable("OIDC_ISSUER is not configured"))?;
        let token = bearer_token(headers)?;
        let provider = CoreProviderMetadata::discover_async(
            IssuerUrl::new(issuer.to_string())
                .map_err(|error| ApiError::unavailable(error.to_string()))?,
            &self.http,
        )
        .await
        .map_err(|error| ApiError::bad_gateway(error.to_string()))?;
        let client = CoreClient::from_provider_metadata(
            provider,
            ClientId::new(self.client_id.clone()),
            None,
        );
        let id_token = CoreIdToken::from_str(&token)
            .map_err(|error| ApiError::unauthorized(error.to_string()))?;
        let claims = id_token
            .claims(&client.id_token_verifier(), no_nonce)
            .map_err(|error| ApiError::unauthorized(error.to_string()))?;

        if let Some(authorized_party) = claims.authorized_party()
            && authorized_party.as_str() != self.client_id
        {
            return Err(ApiError::unauthorized(format!(
                "token authorized party must be {} (found {})",
                self.client_id,
                authorized_party.as_str()
            )));
        }

        Ok(UserInfo {
            subject: claims.subject().as_str().to_string(),
            username: claims
                .preferred_username()
                .map(|username| username.as_str().to_string()),
            email: claims.email().map(|email| email.as_str().to_string()),
        })
    }
}

fn oidc_http_client() -> Result<oidc_reqwest::Client> {
    Ok(oidc_reqwest::ClientBuilder::new()
        .redirect(oidc_reqwest::redirect::Policy::none())
        .build()?)
}

fn no_nonce(nonce: Option<&Nonce>) -> std::result::Result<(), String> {
    match nonce {
        Some(_) => Err("unexpected nonce claim".to_string()),
        None => Ok(()),
    }
}

struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: message.into(),
        }
    }

    fn unavailable(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: message.into(),
        }
    }

    fn bad_gateway(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            message: message.into(),
        }
    }

    fn internal(error: impl std::fmt::Display) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: error.to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        (
            self.status,
            Json(json!({
                "error": self.message,
            })),
        )
            .into_response()
    }
}

fn bearer_token(headers: &HeaderMap) -> Result<String, ApiError> {
    let value = headers
        .get(header::AUTHORIZATION)
        .ok_or_else(|| ApiError::unauthorized("missing Authorization header"))?
        .to_str()
        .map_err(|_| ApiError::unauthorized("invalid Authorization header"))?;
    let token = value
        .strip_prefix("Bearer ")
        .ok_or_else(|| ApiError::unauthorized("Authorization header must use Bearer"))?;
    Ok(token.to_string())
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

pub async fn run() -> Result<()> {
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
        auth: Arc::new(AuthVerifier::new(&config)?),
        config,
        gitops_index,
    };

    let app = Router::new()
        .route("/api/v1/auth/config", get(auth_config))
        .route("/api/v1/me", get(me))
        .route("/api/v1/projects", get(list_projects))
        .route("/api/v1/apps", post(create_app))
        .route(
            "/api/v1/apps/{tenant}/{project}/{environment}",
            delete(delete_app),
        )
        .route("/api/v1/deployments", post(create_deployment))
        .route("/api/v1/workflows/{workflow_id}", get(workflow_status))
        .route("/webhooks/gitea", post(handle_gitea_webhook))
        .route(
            "/healthz",
            get(|| async { StatusCode::NO_CONTENT }).post(|| async { StatusCode::NO_CONTENT }),
        )
        .with_state(state);

    let listener = TcpListener::bind("[::]:8080").await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn auth_config(State(state): State<AppState>) -> Result<Json<ApiAuthConfig>, ApiError> {
    Ok(Json(state.auth.config()?))
}

async fn me(State(state): State<AppState>, headers: HeaderMap) -> Result<Json<UserInfo>, ApiError> {
    Ok(Json(state.auth.verify(&headers).await?))
}

async fn list_projects(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<ProjectSummary>>, ApiError> {
    state.auth.verify(&headers).await?;
    state
        .gitops_index
        .refresh_if_stale()
        .await
        .map_err(ApiError::internal)?;
    let apps = scan_app_inventory(
        &state.gitops_index.config.cache_dir.join("apps"),
        &state.config.registry,
    )
    .map_err(ApiError::internal)?;
    Ok(Json(
        apps.into_iter()
            .map(|app| ProjectSummary {
                tenant: app.tenant,
                project: app.project,
                environment: app.environment,
                resources: app.resources,
                hostnames: app.hostnames,
                images: app.images,
                source_repos: app.source_repos,
            })
            .collect(),
    ))
}

async fn create_app(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateAppRequest>,
) -> Result<Json<WorkflowStarted>, ApiError> {
    state.auth.verify(&headers).await?;
    request.validate().map_err(ApiError::bad_request)?;
    let workflow_id = format!("create-app-{}", sanitize(&request.app_path()));
    workflows::start_create_app_workflow(
        &state.client,
        workflow_id.clone(),
        workflows::create_app::CreateAppInput {
            gitops_url: state.config.gitops_url.clone(),
            gitops_revision: state.config.gitops_revision.clone(),
            registry: state.config.registry.clone(),
            request,
        },
    )
    .await
    .map_err(ApiError::internal)?;

    Ok(Json(WorkflowStarted { workflow_id }))
}

async fn delete_app(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath((tenant, project, environment)): AxumPath<(String, String, String)>,
) -> Result<Json<WorkflowStarted>, ApiError> {
    state.auth.verify(&headers).await?;
    let request = DeleteAppRequest {
        tenant,
        project,
        environment,
    };
    request.validate().map_err(ApiError::bad_request)?;
    let workflow_id = format!("delete-app-{}", sanitize(&request.app_path()));
    workflows::start_delete_app_workflow(
        &state.client,
        workflow_id.clone(),
        workflows::delete_app::DeleteAppInput {
            gitops_url: state.config.gitops_url.clone(),
            gitops_revision: state.config.gitops_revision.clone(),
            registry: state.config.registry.clone(),
            request,
        },
    )
    .await
    .map_err(ApiError::internal)?;

    Ok(Json(WorkflowStarted { workflow_id }))
}

async fn create_deployment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<DeployRequest>,
) -> Result<Json<WorkflowStarted>, ApiError> {
    state.auth.verify(&headers).await?;
    let (owner, repo_name) = request
        .repo
        .split_once('/')
        .ok_or_else(|| ApiError::bad_request("repo must be in owner/name form"))?;
    let source_url = forgejo_clone_url(&state.config, &request.repo);
    let workflow_id = push_workflow_id(repo_name, &request.revision);
    let source = git_source(owner, repo_name, source_url, &request.revision);
    let input = push_to_deploy_input(
        &state.config,
        source,
        request.environment,
        &request.repo,
        &request.revision,
        &workflow_id,
    );

    workflows::start_workflow(&state.client, workflow_id.clone(), input)
        .await
        .map_err(ApiError::internal)?;

    Ok(Json(WorkflowStarted { workflow_id }))
}

async fn workflow_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(workflow_id): AxumPath<String>,
) -> Result<Json<WorkflowStatus>, ApiError> {
    state.auth.verify(&headers).await?;
    let url =
        state.config.temporal_web_url.as_ref().map(|base| {
            temporal_workflow_url(base, &state.config.temporal_namespace, &workflow_id)
        });
    workflows::describe_workflow(&state.client, workflow_id, url)
        .await
        .map(Json)
        .map_err(ApiError::internal)
}

async fn handle_gitea_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    if !is_push_event(&headers) {
        return StatusCode::NO_CONTENT;
    }

    let payload: PushPayload = match serde_json::from_slice(&body) {
        Ok(payload) => payload,
        Err(error) => {
            error!(error = %error, "invalid JSON payload");
            return StatusCode::BAD_REQUEST;
        }
    };

    let owner = payload.repository.owner.username;
    let repo_name = payload.repository.name;
    let revision = payload.after;
    let environment = app_environment(&payload.git_ref, &payload.repository.default_branch);
    let source_repo = format!("{owner}/{repo_name}");

    if state.config.gitops_repo.as_deref() == Some(source_repo.as_str()) {
        if let Err(error) = state.gitops_index.refresh_now().await {
            error!(error = %error, "failed to refresh GitOps deployability cache");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }

        info!(repo = %source_repo, "refreshed GitOps deployability cache after GitOps push");
        return StatusCode::NO_CONTENT;
    }

    if state
        .gitops_index
        .targets_for(&source_repo, &environment)
        .await
        .is_empty()
    {
        info!(
            repo = %source_repo,
            environment = %environment,
            "push is not deployable"
        );
        return StatusCode::NO_CONTENT;
    }

    let workflow_id = push_workflow_id(&repo_name, &revision);
    let input = push_to_deploy_input(
        &state.config,
        git_source(&owner, &repo_name, payload.repository.clone_url, &revision),
        environment,
        &source_repo,
        &revision,
        &workflow_id,
    );

    match workflows::start_workflow(&state.client, workflow_id, input).await {
        Ok(_) => {
            info!(repo = %repo_name, rev = %revision, "push_to_deploy triggered");
            StatusCode::ACCEPTED
        }
        Err(error) => {
            error!(error = %error, "failed to start workflow");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

fn is_push_event(headers: &HeaderMap) -> bool {
    headers
        .get("X-Gitea-Event")
        .or_else(|| headers.get("X-Forgejo-Event"))
        .and_then(|value| value.to_str().ok())
        .is_some_and(|event| event.eq_ignore_ascii_case("push"))
}

fn push_to_deploy_input(
    config: &AppConfig,
    source: Source,
    environment: String,
    repo: &str,
    revision: &str,
    workflow_id: &str,
) -> PushToDeployInput {
    PushToDeployInput {
        source,
        gitops_url: config.gitops_url.clone(),
        gitops_revision: config.gitops_revision.clone(),
        environment,
        registry: config.registry.clone(),
        commit_status: commit_status(config, repo, revision, workflow_id),
    }
}

fn commit_status(
    config: &AppConfig,
    repo: &str,
    sha: &str,
    workflow_id: &str,
) -> Option<ForgejoCommitStatusTarget> {
    let forgejo_url = config.forgejo_url.as_ref()?;
    let temporal_web_url = config.temporal_web_url.as_ref()?;

    Some(ForgejoCommitStatusTarget {
        forgejo_url: forgejo_url.clone(),
        repo: repo.to_string(),
        sha: sha.to_string(),
        target_url: temporal_workflow_url(
            temporal_web_url,
            &config.temporal_namespace,
            workflow_id,
        ),
    })
}

fn git_source(owner: &str, repo_name: &str, url: String, revision: &str) -> Source {
    Source::Git {
        owner: owner.to_string(),
        name: repo_name.to_string(),
        url,
        revision: revision.to_string(),
        path: PathBuf::from(format!("/tmp/{}-{}", sanitize(repo_name), revision)),
    }
}

fn push_workflow_id(repo_name: &str, revision: &str) -> String {
    format!(
        "push-to-deploy-{}-{}",
        sanitize(repo_name),
        &revision[..revision.len().min(12)]
    )
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

fn forgejo_clone_url(config: &AppConfig, repo: &str) -> String {
    let base = config
        .forgejo_public_url
        .as_ref()
        .or(config.forgejo_url.as_ref())
        .map(|url| url.trim_end_matches('/'))
        .unwrap_or("");
    format!("{base}/{repo}.git")
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        AppSourceTarget, AppTarget, app_environment, index_targets, parse_duration,
        repo_slug_from_git_url, temporal_workflow_url,
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
