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
use tokio::{
    fs::{remove_dir_all, write},
    process::Command,
};
use tracing::{info, warn};

const BUILD_CACHE_TAG: &str = "buildcache";

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
            command.args(["buildx", "build"]);
            if docker_build_registry_cache_enabled() {
                let buildx_builder = ensure_buildx_builder(ctx, &image.registry).await?;
                command.args(["--builder", &buildx_builder]);
            }
            configure_docker_build_network(&mut command);
            configure_docker_build_cache(&mut command, &image);
            command
                .args(["--load", "--tag", &image_ref, "."])
                .current_dir(path);
            run_checked_command(ctx, &mut command, "docker buildx build").await?;
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

async fn ensure_buildx_builder(
    ctx: &ActivityContext,
    registry: &str,
) -> Result<String, ActivityError> {
    let builder = docker_buildx_builder_name(registry);
    if buildx_builder_exists(ctx, &builder).await? {
        return Ok(builder);
    }

    let config_path = format!("/tmp/platform-engine-buildkitd-{builder}.toml");
    write(&config_path, buildkitd_config(registry))
        .await
        .map_err(|e| anyhow!("failed to write BuildKit config: {e}"))?;

    let mut command = Command::new("docker");
    command.args([
        "buildx",
        "create",
        "--name",
        &builder,
        "--driver",
        "docker-container",
        "--buildkitd-config",
        &config_path,
        "--buildkitd-flags",
        "--allow-insecure-entitlement network.host",
    ]);
    if let Some(network) = docker_buildx_driver_network() {
        let driver_opt = format!("network={network}");
        command.args(["--driver-opt", &driver_opt]);
    }
    command.arg("--bootstrap");

    let output = run_command(ctx, &mut command, "docker buildx create").await?;
    if output.status.success() || buildx_builder_exists(ctx, &builder).await? {
        return Ok(builder);
    }

    Err(super::process::command_error("docker buildx create", &output).into())
}

async fn buildx_builder_exists(
    ctx: &ActivityContext,
    builder: &str,
) -> Result<bool, ActivityError> {
    let mut command = Command::new("docker");
    command.args(["buildx", "inspect", builder]);
    let output = run_command(ctx, &mut command, "docker buildx inspect").await?;
    Ok(output.status.success())
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
            if network == "host" {
                command.args(["--allow", "network.host"]);
            }
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

fn configure_docker_build_cache(command: &mut Command, image: &Image) {
    if !docker_build_registry_cache_enabled() {
        return;
    }

    let cache_ref = docker_build_cache_ref(image);
    let cache_from = format!("type=registry,ref={cache_ref}");
    let cache_to = format!(
        "type=registry,ref={cache_ref},mode=max,oci-mediatypes=true,image-manifest=true,ignore-error=true"
    );
    command.args(["--cache-from", &cache_from, "--cache-to", &cache_to]);
}

fn docker_build_registry_cache_enabled() -> bool {
    env::var("DOCKER_BUILD_REGISTRY_CACHE").map_or(true, |value| {
        !matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        )
    })
}

fn docker_build_cache_ref(image: &Image) -> String {
    format!(
        "{}/{}/{}:{BUILD_CACHE_TAG}",
        image.registry, image.owner, image.repository
    )
}

fn docker_buildx_builder_name(registry: &str) -> String {
    env::var("DOCKER_BUILDX_BUILDER")
        .ok()
        .map(|builder| builder.trim().to_string())
        .filter(|builder| !builder.is_empty())
        .unwrap_or_else(|| {
            let identity = docker_buildx_driver_network()
                .map(|network| format!("{registry}-{network}"))
                .unwrap_or_else(|| registry.to_string());
            format!("platform-engine-{}", sanitize_buildx_name(&identity))
        })
}

fn docker_buildx_driver_network() -> Option<String> {
    env::var("DOCKER_BUILDX_DRIVER_NETWORK")
        .ok()
        .map(|network| network.trim().to_string())
        .filter(|network| !network.is_empty())
        .or_else(|| {
            env::var("DOCKER_BUILD_NETWORK")
                .ok()
                .map(|network| network.trim().to_string())
                .filter(|network| network == "host")
        })
}

fn buildkitd_config(registry: &str) -> String {
    buildkitd_config_for_registry(registry, buildkit_registry_uses_plain_http(registry))
}

fn buildkitd_config_for_registry(registry: &str, plain_http: bool) -> String {
    let mut config = String::from("insecure-entitlements = [\"network.host\"]\n");
    if plain_http {
        config.push_str(&format!(
            "\n[registry.\"{registry}\"]\n  http = true\n  insecure = true\n"
        ));
    }
    config
}

fn buildkit_registry_uses_plain_http(registry: &str) -> bool {
    env::var("DOCKER_BUILD_REGISTRY_CACHE_HTTP").map_or_else(
        |_| is_local_or_cluster_registry(registry),
        |value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        },
    )
}

fn is_local_or_cluster_registry(registry: &str) -> bool {
    registry == "localhost"
        || registry.starts_with("localhost:")
        || registry.starts_with("127.")
        || registry.ends_with(".svc")
        || registry.contains(".svc.")
}

fn sanitize_buildx_name(input: &str) -> String {
    input
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docker_build_cache_ref_uses_stable_app_tag() {
        let image = Image {
            registry: "registry.registry.svc.cluster.local".to_string(),
            owner: "khuedoan".to_string(),
            repository: "blog".to_string(),
            tag: "revision".to_string(),
        };

        assert_eq!(
            docker_build_cache_ref(&image),
            "registry.registry.svc.cluster.local/khuedoan/blog:buildcache"
        );
    }

    #[test]
    fn buildkitd_config_marks_cluster_registry_as_plain_http() {
        assert_eq!(
            buildkitd_config_for_registry("registry.registry.svc.cluster.local", true),
            "insecure-entitlements = [\"network.host\"]\n\n[registry.\"registry.registry.svc.cluster.local\"]\n  http = true\n  insecure = true\n"
        );
    }

    #[test]
    fn buildkitd_config_leaves_public_registry_on_https() {
        assert_eq!(
            buildkitd_config_for_registry("docker.io", false),
            "insecure-entitlements = [\"network.host\"]\n"
        );
    }

    #[test]
    fn sanitize_buildx_name_removes_registry_separators() {
        assert_eq!(
            sanitize_buildx_name("registry.registry.svc.cluster.local-host"),
            "registry-registry-svc-cluster-local-host"
        );
    }
}
