use super::process::{command_error, run_command};
use crate::core::app::{builder::Builder, image::Image, source::Source};
use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use std::env;
use temporalio_sdk::activities::{ActivityContext, ActivityError};
use tokio::{fs::remove_dir_all, process::Command};
use tracing::{info, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSourcePullInput {
    pub source: Source,
}

pub async fn app_source_pull(
    ctx: ActivityContext,
    input: AppSourcePullInput,
) -> Result<Source, ActivityError> {
    match input.source {
        Source::Git {
            name,
            owner,
            url,
            revision,
            path,
        } => {
            if path.exists() {
                warn!("removing existing workspace at {path:?}");
                remove_dir_all(&path).await.map_err(|e| anyhow!(e))?;
            }

            let path_arg = path.display().to_string();
            let mut command = Command::new("git");
            command.args(["init", &path_arg]);
            let output = run_command(&ctx, &mut command, "git init repository").await?;
            if !output.status.success() {
                return Err(command_error("git init repository", &output).into());
            }

            tokio::fs::create_dir_all(&path)
                .await
                .map_err(|e| anyhow!(e))?;

            let mut command = Command::new("git");
            command.args(["init"]).current_dir(&path);
            let output = run_command(&ctx, &mut command, "git init workspace").await?;
            if !output.status.success() {
                return Err(command_error("git init workspace", &output).into());
            }

            let mut command = Command::new("git");
            command
                .args(["remote", "add", "origin", &url])
                .current_dir(&path);
            let output = run_command(&ctx, &mut command, "git add remote").await?;
            if !output.status.success() {
                return Err(command_error("git add remote", &output).into());
            }

            let mut command = Command::new("git");
            command
                .args(["fetch", "--depth", "1", "origin", &revision])
                .current_dir(&path);
            let output = run_command(&ctx, &mut command, "git fetch").await?;
            if !output.status.success() {
                return Err(command_error("git fetch", &output).into());
            }

            let mut command = Command::new("git");
            command.args(["checkout", &revision]).current_dir(&path);
            let output = run_command(&ctx, &mut command, "git checkout").await?;
            if !output.status.success() {
                return Err(command_error("git checkout", &output).into());
            }

            Ok(Source::Git {
                name,
                owner,
                url,
                revision,
                path,
            })
        }
        Source::Docker(_image) => todo!(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSourceDetectInput {
    pub source: Source,
    pub registry: String,
}

pub async fn app_source_detect(
    ctx: ActivityContext,
    input: AppSourceDetectInput,
) -> Result<Builder, ActivityError> {
    if ctx.is_cancelled() {
        return Err(ActivityError::cancelled());
    }
    Ok(input.source.detect_builder(&input.registry).await?)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppBuildInput {
    pub builder: Builder,
}

pub async fn app_build(ctx: ActivityContext, input: AppBuildInput) -> Result<Image, ActivityError> {
    match input.builder {
        Builder::Dockerfile(path, image) => {
            info!("building container image with Dockerfile");
            let image_ref = format!("{image}");
            let mut command = Command::new("docker");
            command.args(["build", "."]);
            configure_docker_build_network(&mut command);

            let output = run_command(
                &ctx,
                command.args(["--tag", &image_ref]).current_dir(path),
                "docker build",
            )
            .await?;
            if !output.status.success() {
                return Err(command_error("docker build", &output).into());
            }

            Ok(image)
        }
        Builder::Nixpacks(path, image) => {
            info!("building container image with Nixpacks");
            let image_ref = format!("{image}");
            let mut command = Command::new("nixpacks");
            command
                .args(["build", ".", "--tag", &image_ref])
                .current_dir(path);
            let output = run_command(&ctx, &mut command, "nixpacks build").await?;
            if !output.status.success() {
                return Err(command_error("nixpacks build", &output).into());
            }

            Ok(image)
        }
        Builder::Vendor(source_image, _image) => Ok(source_image.rename().await?),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImagePushInput {
    pub image: Image,
}

pub async fn image_push(
    ctx: ActivityContext,
    input: ImagePushInput,
) -> Result<Image, ActivityError> {
    let image_ref = format!("{}", input.image);
    let mut command = Command::new("docker");
    command.args(["push", &image_ref]);
    let output = run_command(&ctx, &mut command, "docker push").await?;
    if !output.status.success() {
        return Err(command_error("docker push", &output).into());
    }

    Ok(input.image)
}

fn configure_docker_build_network(command: &mut Command) {
    if let Ok(network) = env::var("DOCKER_BUILD_NETWORK") {
        let network = network.trim();
        if !network.is_empty() {
            command.args(["--network", network]);
        }
    }

    if let Ok(add_hosts) = env::var("DOCKER_BUILD_ADD_HOSTS") {
        for add_host in add_hosts
            .split(',')
            .map(str::trim)
            .filter(|add_host| !add_host.is_empty())
        {
            command.args(["--add-host", add_host]);
        }
    }
}
