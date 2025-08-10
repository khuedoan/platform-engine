use std::sync::Arc;

use anyhow::Result;
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
    client: Arc<temporal_client::RetryClient<temporal_client::Client>>,
}

#[derive(Deserialize)]
struct RepoInfo {
    name: String,
    #[serde(default)]
    clone_url: String,
}

#[derive(Deserialize)]
struct PushPayload {
    after: String,
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
    };

    let app = Router::new()
        .route("/webhooks/gitea", post(handle_gitea_webhook))
        .route("/healthz", post(|| async { StatusCode::NO_CONTENT }))
        .with_state(state);

    let listener = TcpListener::bind("0.0.0.0:8080").await?;
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
            let repo_name = payload.repository.name;
            let revision = payload.after;
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
                url,
                revision: revision.clone(),
                path: std::path::PathBuf::from(workspace_path),
            };

            let workflow_id = format!(
                "golden-{}-{}",
                sanitize(&repo_name),
                &revision[..std::cmp::min(12, revision.len())]
            );

            // Start workflow
            let start_res = workflows::start_workflow(&state.client, workflow_id, source).await;
            match start_res {
                Ok(_) => {
                    info!(repo = %repo_name, rev = %revision, "golden_path triggered");
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
