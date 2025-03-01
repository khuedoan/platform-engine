use anyhow::Result;
use app_engine::{activities, temporal, workflows};
use std::sync::Arc;
use temporal_sdk::Worker;
use temporal_sdk_core::{init_worker, CoreRuntime, WorkerConfigBuilder};
use temporal_sdk_core_api::telemetry::TelemetryOptionsBuilder;
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
        .worker_build_id("v0.1.0")
        .build()?;
    let core_worker = init_worker(&runtime, worker_config, client)?;
    let mut worker = Worker::new_from_core(Arc::new(core_worker), "main");

    worker.register_activity(
        activities::app_source_pull::name(),
        activities::app_source_pull::run,
    );
    worker.register_activity(
        activities::app_source_detect::name(),
        activities::app_source_detect::run,
    );
    worker.register_activity(activities::app_build::name(), activities::app_build::run);
    worker.register_wf(workflows::golden_path::name(), workflows::golden_path::run);

    worker.run().await?;

    Ok(())
}
