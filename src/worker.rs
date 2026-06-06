use crate::{activities::*, temporal, workflows};
use anyhow::Result;
use std::{env, fs, time::Duration};
use temporalio_common::worker::WorkerDeploymentOptions;
use temporalio_sdk::{Worker, WorkerOptions};
use temporalio_sdk_core::{CoreRuntime, RuntimeOptions};
use tracing::info;
use tracing_subscriber::EnvFilter;

pub async fn run() -> Result<()> {
    tracing_subscriber::fmt::fmt()
        .with_env_filter(EnvFilter::try_from_env("LOG_LEVEL").unwrap_or(EnvFilter::new("info")))
        .without_time()
        .init();

    let client = temporal::get_client().await?;
    let task_queue = env::var("TASK_QUEUE").unwrap_or_else(|_| "main".to_string());
    let worker_identity = worker_identity();
    let deployment_options =
        WorkerDeploymentOptions::from_build_id(env!("CARGO_PKG_VERSION").to_string());

    let runtime_options = RuntimeOptions::builder()
        .build()
        .map_err(anyhow::Error::msg)?;
    let runtime = CoreRuntime::new_assume_tokio(runtime_options)?;
    let worker_options = WorkerOptions::new(task_queue.clone())
        .client_identity_override(worker_identity)
        .deployment_options(deployment_options)
        .register_activities(PlatformActivities);

    let worker_options = match task_queue.as_str() {
        "bootstrap" => {
            sync_forgejo_bootstrap_schedule(&client, task_queue.clone()).await?;
            worker_options
                .register_workflow::<workflows::forgejo_bootstrap::ForgejoBootstrapWorkflow>()
                .build()
        }
        _ => worker_options
            .register_workflow::<workflows::create_app::CreateAppWorkflow>()
            .register_workflow::<workflows::delete_app::DeleteAppWorkflow>()
            .register_workflow::<workflows::push_to_deploy::PushToDeployWorkflow>()
            .register_workflow::<workflows::gitops_publish::GitopsPublishWorkflow>()
            .build(),
    };

    let mut worker =
        Worker::new(&runtime, client, worker_options).map_err(|err| anyhow::anyhow!("{err}"))?;
    worker.run().await?;

    Ok(())
}

fn worker_identity() -> String {
    fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .or_else(|| env::var("HOSTNAME").ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "netamos-worker".to_string())
}

async fn sync_forgejo_bootstrap_schedule(
    client: &temporalio_client::Client,
    task_queue: String,
) -> Result<()> {
    match env::var("FORGEJO_BOOTSTRAP").ok().as_deref() {
        Some("false") => {
            workflows::delete_forgejo_bootstrap_schedule(client).await?;
            info!("disabled Forgejo bootstrap Temporal schedule");
        }
        Some(_) => {
            let interval = env::var("FORGEJO_BOOTSTRAP_INTERVAL_SECONDS")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(86_400);
            let input = workflows::forgejo_bootstrap::ForgejoBootstrapInput::from_env();

            workflows::ensure_forgejo_bootstrap_schedule(
                client,
                task_queue,
                input,
                Duration::from_secs(interval),
            )
            .await?;
            info!("synced Forgejo bootstrap Temporal schedule");
        }
        None => {}
    }

    Ok(())
}
