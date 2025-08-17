use anyhow::Result;
use gethostname::gethostname;
use platform_engine::{activities::*, temporal, workflows};
use std::sync::Arc;
use temporal_sdk::Worker;
use temporal_sdk_core::{CoreRuntime, WorkerConfigBuilder, init_worker};
use temporal_sdk_core_api::{telemetry::TelemetryOptionsBuilder, worker::WorkerVersioningStrategy};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::fmt()
        .with_env_filter(EnvFilter::try_from_env("LOG_LEVEL").unwrap_or(EnvFilter::new("info")))
        .without_time()
        .init();

    let client = temporal::get_client().await?;

    let telemetry_options = TelemetryOptionsBuilder::default().build()?;
    let runtime = CoreRuntime::new_assume_tokio(telemetry_options)?;

    let worker_config = WorkerConfigBuilder::default()
        .namespace("default")
        .task_queue("main")
        .client_identity_override(Some(gethostname().to_string_lossy().into_owned()))
        .versioning_strategy(WorkerVersioningStrategy::None {
            build_id: env!("CARGO_PKG_VERSION").to_string(),
        })
        .build()?;
    let core_worker = init_worker(&runtime, worker_config, client)?;
    let mut worker = Worker::new_from_core(Arc::new(core_worker), "main");

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

    worker.run().await?;

    Ok(())
}
