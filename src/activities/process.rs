use anyhow::anyhow;
use std::process::{Output, Stdio};
use temporalio_sdk::activities::{ActivityContext, ActivityError};
use tokio::{
    process::Command,
    time::{Duration, MissedTickBehavior},
};

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);

pub async fn run_command(
    ctx: &ActivityContext,
    command: &mut Command,
    operation: &str,
) -> Result<Output, ActivityError> {
    if ctx.is_cancelled() {
        return Err(ActivityError::cancelled());
    }

    ctx.record_heartbeat(vec![]);
    command.kill_on_drop(true);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = command
        .spawn()
        .map_err(|e| anyhow!("failed to start {operation}: {e}"))?;
    let output = child.wait_with_output();
    tokio::pin!(output);

    let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            result = &mut output => return result.map_err(ActivityError::from),
            _ = heartbeat.tick() => ctx.record_heartbeat(vec![]),
            _ = ctx.cancelled() => return Err(ActivityError::cancelled()),
        }
    }
}

pub async fn run_checked_command(
    ctx: &ActivityContext,
    command: &mut Command,
    operation: &str,
) -> Result<Output, ActivityError> {
    let output = run_command(ctx, command, operation).await?;
    if !output.status.success() {
        return Err(command_error(operation, &output).into());
    }

    Ok(output)
}

pub async fn run_stdout_command(
    ctx: &ActivityContext,
    command: &mut Command,
    operation: &str,
) -> Result<String, ActivityError> {
    let output = run_checked_command(ctx, command, operation).await?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn command_error(operation: &str, output: &Output) -> anyhow::Error {
    anyhow!(
        "{operation} failed\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    )
}
