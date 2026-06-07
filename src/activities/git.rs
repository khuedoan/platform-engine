use super::{
    forgejo::ForgejoCommitStatusTarget,
    git_auth::git_command_for_url,
    process::{run_checked_command, run_stdout_command},
    workspace::TempWorkspace,
};
use crate::{
    api::{CreateAppRequest, DeleteAppRequest},
    core::app::image::Image,
    gitops::{
        AppImageUpdate, AppsBundle, UpdateAppVersionInput, scan_app_source_targets,
        update_app_version_inner, write_apps_bundle, write_create_app_manifests,
    },
};
use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, env, fs, path::Path};
use temporalio_sdk::{
    ApplicationFailure,
    activities::{ActivityContext, ActivityError},
};
use tokio::{fs::remove_dir_all, process::Command};
use tracing::info;

pub use crate::gitops::AppTarget;

const APPS_REPOSITORY: &str = "apps";
const APPS_TAG: &str = "latest";

fn non_retryable_error(error: anyhow::Error) -> ActivityError {
    ActivityError::application(ApplicationFailure::non_retryable(error))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateGitopsImageInput {
    pub url: String,
    pub revision: String,
    pub source_repo: String,
    pub environment: String,
    pub image: Image,
    #[serde(default)]
    pub commit_status: Option<ForgejoCommitStatusTarget>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateGitopsImageResult {
    pub changed: bool,
    pub commit_sha: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnqueueGitopsPublishInput {
    pub workflow_id: String,
    pub update: UpdateGitopsImageInput,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindGitopsAppTargetsInput {
    pub url: String,
    pub revision: String,
    pub registry: String,
    pub source_repo: String,
    pub environment: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindGitopsSourceReposInput {
    pub url: String,
    pub revision: String,
    pub registry: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateGitopsAppInput {
    pub url: String,
    pub revision: String,
    pub registry: String,
    pub request: CreateAppRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateGitopsAppResult {
    pub changed: bool,
    pub commit_sha: Option<String>,
    pub app_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteGitopsAppInput {
    pub url: String,
    pub revision: String,
    pub registry: String,
    pub request: DeleteAppRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteGitopsAppResult {
    pub changed: bool,
    pub commit_sha: Option<String>,
    pub app_path: String,
}

pub async fn enqueue_gitops_publish(
    ctx: ActivityContext,
    input: EnqueueGitopsPublishInput,
) -> Result<(), ActivityError> {
    if ctx.is_cancelled() {
        return Err(ActivityError::cancelled());
    }

    ctx.record_heartbeat(vec![]);
    let client = crate::temporal::get_client().await?;
    crate::workflows::signal_gitops_publish(&client, input.workflow_id, input.update)
        .await
        .map_err(ActivityError::from)
}

pub async fn find_gitops_app_targets(
    ctx: ActivityContext,
    input: FindGitopsAppTargetsInput,
) -> Result<Vec<AppTarget>, ActivityError> {
    let workspace = TempWorkspace::new("gitops-targets", &input.url, &input.revision);
    clone_repo(&ctx, &input.url, &input.revision, workspace.path()).await?;

    let targets = scan_app_source_targets(&workspace.path().join("apps"), &input.registry)?
        .into_iter()
        .filter(|mapping| {
            mapping.source_repo == input.source_repo
                && mapping.target.environment == input.environment
        })
        .map(|mapping| mapping.target)
        .collect();

    Ok(targets)
}

pub async fn find_gitops_source_repos(
    ctx: ActivityContext,
    input: FindGitopsSourceReposInput,
) -> Result<Vec<String>, ActivityError> {
    let workspace = TempWorkspace::new("gitops-sources", &input.url, &input.revision);
    clone_repo(&ctx, &input.url, &input.revision, workspace.path()).await?;

    let repos = scan_app_source_targets(&workspace.path().join("apps"), &input.registry)?
        .into_iter()
        .map(|mapping| mapping.source_repo)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    Ok(repos)
}

pub async fn update_gitops_image(
    ctx: ActivityContext,
    input: UpdateGitopsImageInput,
) -> Result<UpdateGitopsImageResult, ActivityError> {
    let workspace = TempWorkspace::new("gitops", &input.url, &input.revision);
    clone_repo(&ctx, &input.url, &input.revision, workspace.path()).await?;
    configure_git_user(&ctx, workspace.path()).await?;

    let apps_dir = workspace.path().join("apps");
    let repository = format!(
        "{}/{}/{}",
        input.image.registry, input.image.owner, input.image.repository
    );
    let changed = update_app_version_inner(UpdateAppVersionInput {
        apps_dir: apps_dir.to_string_lossy().to_string(),
        environment: input.environment.clone(),
        new_images: vec![AppImageUpdate {
            repository,
            tag: input.image.tag.clone(),
        }],
    })
    .await?;

    let mut commit_sha = None;
    if changed {
        let commit_message = format!(
            "chore(apps): update {} image for {}",
            input.source_repo, input.environment
        );
        commit_sha = Some(
            commit_and_push_gitops(
                &ctx,
                workspace.path(),
                &input.url,
                &input.revision,
                &commit_message,
            )
            .await?,
        );
    }

    let bundle_workspace = TempWorkspace::new("apps-bundle", &input.url, &input.revision);
    let bundle = write_apps_bundle(
        bundle_workspace.path(),
        &apps_dir,
        APPS_REPOSITORY,
        APPS_TAG,
        &input.image.registry,
    )?;
    push_apps_bundle(&ctx, &input.image.registry, &bundle).await?;

    Ok(UpdateGitopsImageResult {
        changed,
        commit_sha,
    })
}

pub async fn create_gitops_app(
    ctx: ActivityContext,
    input: CreateGitopsAppInput,
) -> Result<CreateGitopsAppResult, ActivityError> {
    if ctx.is_cancelled() {
        return Err(ActivityError::cancelled());
    }

    input
        .request
        .validate()
        .map_err(|error| non_retryable_error(anyhow!(error)))?;

    let workspace = TempWorkspace::new("create-app", &input.url, &input.revision);
    clone_repo(&ctx, &input.url, &input.revision, workspace.path()).await?;
    configure_git_user(&ctx, workspace.path()).await?;

    let app_path = input.request.app_path();
    let apps_dir = workspace.path().join("apps");
    let app_dir = apps_dir
        .join(&input.request.tenant)
        .join(&input.request.project)
        .join(&input.request.environment);

    if app_dir.exists() {
        if !input.request.force {
            return Err(non_retryable_error(anyhow!(
                "apps/{app_path} already exists; pass force to replace it"
            )));
        }
        fs::remove_dir_all(&app_dir)?;
    }
    fs::create_dir_all(&app_dir)?;
    write_create_app_manifests(&app_dir, &input.request, &input.registry)?;

    let pathspec = format!("apps/{app_path}");
    let changed = git_has_changes(&ctx, workspace.path(), &pathspec).await?;
    let commit_sha = if changed {
        let commit_message = format!("feat(apps): create {app_path}");
        Some(
            commit_and_push_gitops(
                &ctx,
                workspace.path(),
                &input.url,
                &input.revision,
                &commit_message,
            )
            .await?,
        )
    } else {
        None
    };

    let bundle_workspace = TempWorkspace::new("apps-bundle", &input.url, &input.revision);
    let bundle = write_apps_bundle(
        bundle_workspace.path(),
        &apps_dir,
        APPS_REPOSITORY,
        APPS_TAG,
        &input.registry,
    )?;
    push_apps_bundle(&ctx, &input.registry, &bundle).await?;

    Ok(CreateGitopsAppResult {
        changed,
        commit_sha,
        app_path,
    })
}

pub async fn delete_gitops_app(
    ctx: ActivityContext,
    input: DeleteGitopsAppInput,
) -> Result<DeleteGitopsAppResult, ActivityError> {
    if ctx.is_cancelled() {
        return Err(ActivityError::cancelled());
    }

    input
        .request
        .validate()
        .map_err(|error| non_retryable_error(anyhow!(error)))?;

    let workspace = TempWorkspace::new("delete-app", &input.url, &input.revision);
    clone_repo(&ctx, &input.url, &input.revision, workspace.path()).await?;
    configure_git_user(&ctx, workspace.path()).await?;

    let app_path = input.request.app_path();
    let apps_dir = workspace.path().join("apps");
    let app_dir = apps_dir
        .join(&input.request.tenant)
        .join(&input.request.project)
        .join(&input.request.environment);

    let removed = if app_dir.exists() {
        fs::remove_dir_all(&app_dir)?;
        true
    } else {
        false
    };

    let pathspec = format!("apps/{app_path}");
    let changed = removed && git_has_changes(&ctx, workspace.path(), &pathspec).await?;
    let commit_sha = if changed {
        let commit_message = format!("chore(apps): delete {app_path}");
        Some(
            commit_and_push_gitops(
                &ctx,
                workspace.path(),
                &input.url,
                &input.revision,
                &commit_message,
            )
            .await?,
        )
    } else {
        None
    };

    let bundle_workspace = TempWorkspace::new("apps-bundle", &input.url, &input.revision);
    let bundle = write_apps_bundle(
        bundle_workspace.path(),
        &apps_dir,
        APPS_REPOSITORY,
        APPS_TAG,
        &input.registry,
    )?;
    push_apps_bundle(&ctx, &input.registry, &bundle).await?;

    Ok(DeleteGitopsAppResult {
        changed,
        commit_sha,
        app_path,
    })
}

async fn commit_and_push_gitops(
    ctx: &ActivityContext,
    workspace: &Path,
    url: &str,
    revision: &str,
    commit_message: &str,
) -> Result<String, ActivityError> {
    let mut command = Command::new("git");
    command.args(["add", "apps"]).current_dir(workspace);
    run_checked_command(ctx, &mut command, "git add app version").await?;

    let mut command = Command::new("git");
    command
        .args(["commit", "-m", commit_message])
        .current_dir(workspace);
    run_checked_command(ctx, &mut command, "git commit app version").await?;

    let mut command = Command::new("git");
    command.args(["rev-parse", "HEAD"]).current_dir(workspace);
    let commit_sha = run_stdout_command(ctx, &mut command, "git rev-parse HEAD").await?;

    let git_username = env::var("GIT_USERNAME").unwrap_or_else(|_| "git".to_string());
    let git_password = env::var("GIT_PASSWORD").unwrap_or_else(|_| "password".to_string());
    let mut command = git_command_for_url(url, &git_username, &git_password);
    let branch_ref = format!("HEAD:{revision}");
    command
        .args(["push", "origin", &branch_ref])
        .current_dir(workspace);
    run_checked_command(ctx, &mut command, "git push app version").await?;

    Ok(commit_sha)
}

async fn git_has_changes(
    ctx: &ActivityContext,
    workspace: &Path,
    pathspec: &str,
) -> Result<bool, ActivityError> {
    let mut command = Command::new("git");
    command
        .args(["status", "--porcelain", "--", pathspec])
        .current_dir(workspace);
    let status = run_stdout_command(ctx, &mut command, "git status app").await?;
    Ok(!status.trim().is_empty())
}

async fn clone_repo(
    ctx: &ActivityContext,
    url: &str,
    revision: &str,
    workspace: &Path,
) -> Result<(), ActivityError> {
    if workspace.exists() {
        remove_dir_all(workspace)
            .await
            .map_err(|error| anyhow!(error))?;
    }

    let git_username = env::var("GIT_USERNAME")
        .or_else(|_| env::var("NETAMOS_USERNAME"))
        .unwrap_or_else(|_| "git".to_string());
    let git_password = env::var("GIT_PASSWORD")
        .or_else(|_| env::var("NETAMOS_PASSWORD"))
        .unwrap_or_else(|_| "password".to_string());
    let mut command = git_command_for_url(url, &git_username, &git_password);
    command
        .args(["clone", "--branch", revision, url])
        .arg(workspace);
    run_checked_command(ctx, &mut command, "git clone GitOps repo").await?;
    Ok(())
}

async fn configure_git_user(ctx: &ActivityContext, workspace: &Path) -> Result<(), ActivityError> {
    let git_user = env::var("GIT_USER").unwrap_or_else(|_| "Platform Engine".to_string());
    let git_email = env::var("GIT_EMAIL").unwrap_or_else(|_| "platform@example.com".to_string());

    let mut command = Command::new("git");
    command
        .args(["config", "user.name", &git_user])
        .current_dir(workspace);
    run_checked_command(ctx, &mut command, "git config user.name").await?;

    let mut command = Command::new("git");
    command
        .args(["config", "user.email", &git_email])
        .current_dir(workspace);
    run_checked_command(ctx, &mut command, "git config user.email").await?;

    Ok(())
}

async fn push_apps_bundle(
    ctx: &ActivityContext,
    registry: &str,
    bundle: &AppsBundle,
) -> Result<(), ActivityError> {
    info!(
        artifacts = bundle.apps.len() + 1,
        manifests = bundle.count,
        "pushing apps OCI artifacts"
    );

    for app in &bundle.apps {
        push_flux_artifact(
            ctx,
            registry,
            &app.repository,
            APPS_TAG,
            &app.name,
            &app.dir,
        )
        .await?;
    }
    push_flux_artifact(
        ctx,
        registry,
        APPS_REPOSITORY,
        APPS_TAG,
        APPS_REPOSITORY,
        &bundle.root_dir,
    )
    .await?;

    Ok(())
}

async fn push_flux_artifact(
    ctx: &ActivityContext,
    registry: &str,
    repository: &str,
    revision: &str,
    source: &str,
    path: &Path,
) -> Result<(), ActivityError> {
    let artifact_url = format!("oci://{registry}/{repository}:{revision}");
    let path = path.to_string_lossy().to_string();

    info!(artifact = %artifact_url, path = %path, "pushing Flux OCI artifact");

    let mut command = Command::new("flux");
    command.args([
        "push",
        "artifact",
        &artifact_url,
        "--path",
        &path,
        "--source",
        source,
        "--revision",
        revision,
        "--insecure-registry",
    ]);
    run_checked_command(ctx, &mut command, "flux push artifact").await?;

    Ok(())
}
