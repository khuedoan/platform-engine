use anyhow::Result;
use gethostname::gethostname;
use platform_engine::{activities::*, temporal, workflows};
use std::{env, sync::Arc, time::Duration};
use temporal_sdk::Worker;
use temporal_sdk_core::{CoreRuntime, WorkerConfigBuilder, init_worker};
use temporal_sdk_core_api::{telemetry::TelemetryOptionsBuilder, worker::WorkerVersioningStrategy};
use tokio::time::sleep;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::fmt()
        .with_env_filter(EnvFilter::try_from_env("LOG_LEVEL").unwrap_or(EnvFilter::new("info")))
        .without_time()
        .init();

    let client = temporal::get_client().await?;
    let task_queue = env::var("TASK_QUEUE").unwrap_or_else(|_| "main".to_string());

    let telemetry_options = TelemetryOptionsBuilder::default().build()?;
    let runtime = CoreRuntime::new_assume_tokio(telemetry_options)?;

    let worker_config = WorkerConfigBuilder::default()
        .namespace("default")
        .task_queue(task_queue.clone())
        .client_identity_override(Some(gethostname().to_string_lossy().into_owned()))
        .versioning_strategy(WorkerVersioningStrategy::None {
            build_id: env!("CARGO_PKG_VERSION").to_string(),
        })
        .build()?;
    let core_worker = init_worker(&runtime, worker_config, client)?;
    let mut worker = Worker::new_from_core(Arc::new(core_worker), &task_queue);

    match task_queue.as_str() {
        "bootstrap" => {
            worker.register_activity("forgejo_wait", forgejo_wait);
            worker.register_activity("forgejo_ensure_user", forgejo_ensure_user);
            worker.register_activity("forgejo_ensure_repo", forgejo_ensure_repo);
            worker.register_activity("forgejo_ensure_webhook", forgejo_ensure_webhook);
            worker.register_activity("forgejo_ensure_collaborator", forgejo_ensure_collaborator);
            worker.register_activity(
                "forgejo_ensure_gitops_repo_seeded",
                forgejo_ensure_gitops_repo_seeded,
            );
            worker.register_wf(
                workflows::forgejo_bootstrap::name(),
                workflows::forgejo_bootstrap::definition,
            );
            start_forgejo_bootstrap_loop(task_queue.clone());
        }
        _ => {
            worker.register_activity("app_source_pull", app_source_pull);
            worker.register_activity("app_source_detect", app_source_detect);
            worker.register_activity("app_build", app_build);
            worker.register_activity("image_push", image_push);
            worker.register_activity("clone", clone);
            worker.register_activity("update_app_version", update_app_version);
            worker.register_activity("git_add", git_add);
            worker.register_activity("git_commit", git_commit);
            worker.register_activity("git_push", git_push);
            worker.register_wf(
                workflows::push_to_deploy::name(),
                workflows::push_to_deploy::definition,
            );
        }
    }

    worker.run().await?;

    Ok(())
}

fn start_forgejo_bootstrap_loop(task_queue: String) {
    const WORKFLOW_ID: &str = "forgejo-bootstrap";

    if env::var("FORGEJO_BOOTSTRAP")
        .map(|value| value != "false")
        .unwrap_or(false)
    {
        tokio::spawn(async move {
            let interval = env::var("FORGEJO_BOOTSTRAP_INTERVAL_SECONDS")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(1800);

            loop {
                let input = workflows::forgejo_bootstrap::ForgejoBootstrapInput::from_env();

                match temporal::get_client().await {
                    Ok(client) => {
                        if let Err(err) = workflows::start_forgejo_bootstrap(
                            &client,
                            WORKFLOW_ID.to_string(),
                            task_queue.clone(),
                            input,
                        )
                        .await
                        {
                            error!(error = %err, "failed to start Forgejo bootstrap workflow");
                        } else {
                            info!("started Forgejo bootstrap workflow");
                        }
                    }
                    Err(err) => error!(error = %err, "failed to connect to Temporal"),
                }

                sleep(Duration::from_secs(interval)).await;
            }
        });
    }
}
