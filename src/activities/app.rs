use super::{
    git_auth::git_command_for_url,
    process::{run_checked_command, run_command},
    workspace::TempWorkspace,
};
use crate::core::app::{builder::Builder, image::Image, source::Source};
use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use std::env;
use temporalio_sdk::activities::{ActivityContext, ActivityError};
use tokio::{fs::remove_dir_all, process::Command};
use tracing::{info, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishImageFromSourceInput {
    pub source: Source,
    pub registry: String,
}

pub async fn publish_image_from_source(
    ctx: ActivityContext,
    input: PublishImageFromSourceInput,
) -> Result<Image, ActivityError> {
    let (_workspace, source) = source_with_activity_workspace(input.source);
    let source = pull_source(&ctx, source).await?;
    let builder = detect_builder(&ctx, &source, &input.registry).await?;
    let image = builder_image(&builder);

    if image_exists_in_registry(&ctx, &image).await? {
        info!(image = %image, "image already exists in registry");
        return Ok(image);
    }

    let image = build_image(&ctx, builder).await?;
    push_image(&ctx, &image).await?;
    Ok(image)
}

fn source_with_activity_workspace(source: Source) -> (Option<TempWorkspace>, Source) {
    match source {
        Source::Git {
            name,
            owner,
            url,
            revision,
            ..
        } => {
            let workspace = TempWorkspace::new("source", &url, &revision);
            let path = workspace.path().to_path_buf();
            (
                Some(workspace),
                Source::Git {
                    name,
                    owner,
                    url,
                    revision,
                    path,
                },
            )
        }
        Source::Docker(image) => (None, Source::Docker(image)),
    }
}

async fn pull_source(ctx: &ActivityContext, source: Source) -> Result<Source, ActivityError> {
    match source {
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

            tokio::fs::create_dir_all(&path)
                .await
                .map_err(|e| anyhow!(e))?;

            let mut command = Command::new("git");
            command.args(["init"]).current_dir(&path);
            run_checked_command(ctx, &mut command, "git init workspace").await?;

            let mut command = Command::new("git");
            command
                .args(["remote", "add", "origin", &url])
                .current_dir(&path);
            run_checked_command(ctx, &mut command, "git add remote").await?;

            let git_username = env::var("GIT_USERNAME").unwrap_or_else(|_| "git".to_string());
            let git_password = env::var("GIT_PASSWORD").unwrap_or_else(|_| "password".to_string());
            let mut command = git_command_for_url(&url, &git_username, &git_password);
            command
                .args(["fetch", "--depth", "1", "origin", &revision])
                .current_dir(&path);
            run_checked_command(ctx, &mut command, "git fetch").await?;

            let mut command = Command::new("git");
            command.args(["checkout", "FETCH_HEAD"]).current_dir(&path);
            run_checked_command(ctx, &mut command, "git checkout").await?;

            Ok(Source::Git {
                name,
                owner,
                url,
                revision,
                path,
            })
        }
        Source::Docker(image) => Ok(Source::Docker(image)),
    }
}

async fn detect_builder(
    ctx: &ActivityContext,
    source: &Source,
    registry: &str,
) -> Result<Builder, ActivityError> {
    if ctx.is_cancelled() {
        return Err(ActivityError::cancelled());
    }

    match source {
        Source::Git {
            name,
            owner,
            revision,
            path,
            ..
        } => {
            let image = Image {
                registry: registry.to_owned(),
                owner: owner.to_string(),
                repository: name.to_string(),
                tag: revision.to_string(),
            };

            if path.join("Dockerfile").exists() {
                return Ok(Builder::Dockerfile(path.to_path_buf(), image));
            }

            let mut command = Command::new("nixpacks");
            command.args(["detect", "."]).current_dir(path);
            let output = run_command(ctx, &mut command, "nixpacks detect").await?;
            if output.status.success() && output.stdout.len() > 1 {
                Ok(Builder::Nixpacks(path.to_path_buf(), image))
            } else {
                Err(anyhow!("no buildable code detected").into())
            }
        }
        Source::Docker(image) => Ok(Builder::Vendor(
            image.clone(),
            Image {
                registry: registry.to_owned(),
                owner: image.owner.clone(),
                repository: image.repository.clone(),
                tag: image.tag.clone(),
            },
        )),
    }
}

fn builder_image(builder: &Builder) -> Image {
    match builder {
        Builder::Dockerfile(_, image) | Builder::Nixpacks(_, image) => image.clone(),
        Builder::Vendor(_, image) => image.clone(),
    }
}

async fn image_exists_in_registry(
    ctx: &ActivityContext,
    image: &Image,
) -> Result<bool, ActivityError> {
    let image_ref = format!("{image}");
    let mut command = Command::new("docker");
    command.args(["manifest", "inspect", &image_ref]);
    let output = run_command(ctx, &mut command, "docker manifest inspect").await?;
    Ok(output.status.success())
}

async fn build_image(ctx: &ActivityContext, builder: Builder) -> Result<Image, ActivityError> {
    match builder {
        Builder::Dockerfile(path, image) => {
            info!("building container image with Dockerfile");
            let image_ref = format!("{image}");
            let mut command = Command::new("docker");
            command.args(["build", "."]);
            configure_docker_build_network(&mut command);
            command.args(["--tag", &image_ref]).current_dir(path);
            run_checked_command(ctx, &mut command, "docker build").await?;
            Ok(image)
        }
        Builder::Nixpacks(path, image) => {
            info!("building container image with Nixpacks");
            let image_ref = format!("{image}");
            let mut command = Command::new("nixpacks");
            command
                .args(["build", ".", "--tag", &image_ref])
                .current_dir(path);
            run_checked_command(ctx, &mut command, "nixpacks build").await?;
            Ok(image)
        }
        Builder::Vendor(source_image, image) => {
            let source_ref = format!("{source_image}");
            let image_ref = format!("{image}");

            let mut command = Command::new("docker");
            command.args(["pull", &source_ref]);
            run_checked_command(ctx, &mut command, "docker pull source image").await?;

            let mut command = Command::new("docker");
            command.args(["tag", &source_ref, &image_ref]);
            run_checked_command(ctx, &mut command, "docker tag source image").await?;

            Ok(image)
        }
    }
}

async fn push_image(ctx: &ActivityContext, image: &Image) -> Result<(), ActivityError> {
    let image_ref = format!("{image}");
    let mut command = Command::new("docker");
    command.args(["push", &image_ref]);
    run_checked_command(ctx, &mut command, "docker push").await?;
    Ok(())
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
