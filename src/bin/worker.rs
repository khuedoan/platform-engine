use anyhow::Result;
use gethostname::gethostname;
use platform_engine::{activities::*, temporal, workflows};
use std::{env, time::Duration};
use temporalio_common::worker::WorkerDeploymentOptions;
use temporalio_sdk::{Worker, WorkerOptions};
use temporalio_sdk_core::{CoreRuntime, RuntimeOptions};
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
    let worker_identity = gethostname().to_string_lossy().into_owned();
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
            start_forgejo_bootstrap_loop(task_queue.clone());
            worker_options
                .register_workflow::<workflows::forgejo_bootstrap::ForgejoBootstrapWorkflow>()
                .build()
        }
        _ => worker_options
            .register_workflow::<workflows::push_to_deploy::PushToDeployWorkflow>()
            .build(),
    };

    let mut worker =
        Worker::new(&runtime, client, worker_options).map_err(|err| anyhow::anyhow!("{err}"))?;
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
