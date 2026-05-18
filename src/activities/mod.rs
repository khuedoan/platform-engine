mod app;
mod forgejo;
mod git;

pub use app::*;
pub use forgejo::*;
pub use git::*;

use crate::core::app::{builder::Builder, image::Image, source::Source};
use temporalio_macros::activities;
use temporalio_sdk::activities::{ActivityContext, ActivityError};

#[derive(Clone, Copy)]
pub struct PlatformActivities;

#[activities]
impl PlatformActivities {
    #[activity]
    pub async fn app_source_pull(
        ctx: ActivityContext,
        input: AppSourcePullInput,
    ) -> Result<Source, ActivityError> {
        app_source_pull(ctx, input).await
    }

    #[activity]
    pub async fn app_source_detect(
        ctx: ActivityContext,
        input: AppSourceDetectInput,
    ) -> Result<Builder, ActivityError> {
        app_source_detect(ctx, input).await
    }

    #[activity]
    pub async fn app_build(
        ctx: ActivityContext,
        input: AppBuildInput,
    ) -> Result<Image, ActivityError> {
        app_build(ctx, input).await
    }

    #[activity]
    pub async fn image_push(
        ctx: ActivityContext,
        input: ImagePushInput,
    ) -> Result<Image, ActivityError> {
        image_push(ctx, input).await
    }

    #[activity]
    pub async fn clone(ctx: ActivityContext, input: CloneInput) -> Result<String, ActivityError> {
        clone(ctx, input).await
    }

    #[activity]
    pub async fn update_app_version(
        ctx: ActivityContext,
        input: UpdateAppVersionInput,
    ) -> Result<bool, ActivityError> {
        update_app_version(ctx, input).await
    }

    #[activity]
    pub async fn git_add(ctx: ActivityContext, input: GitAddInput) -> Result<(), ActivityError> {
        git_add(ctx, input).await
    }

    #[activity]
    pub async fn git_commit(
        ctx: ActivityContext,
        input: GitCommitInput,
    ) -> Result<(), ActivityError> {
        git_commit(ctx, input).await
    }

    #[activity]
    pub async fn git_push(ctx: ActivityContext, input: GitPushInput) -> Result<(), ActivityError> {
        git_push(ctx, input).await
    }

    #[activity]
    pub async fn forgejo_wait(
        ctx: ActivityContext,
        forgejo_url: String,
    ) -> Result<(), ActivityError> {
        forgejo_wait(ctx, forgejo_url).await
    }

    #[activity]
    pub async fn forgejo_ensure_user(
        ctx: ActivityContext,
        input: ForgejoEnsureUserInput,
    ) -> Result<(), ActivityError> {
        forgejo_ensure_user(ctx, input).await
    }

    #[activity]
    pub async fn forgejo_ensure_repo(
        ctx: ActivityContext,
        input: ForgejoEnsureRepoInput,
    ) -> Result<(), ActivityError> {
        forgejo_ensure_repo(ctx, input).await
    }

    #[activity]
    pub async fn forgejo_ensure_webhook(
        ctx: ActivityContext,
        input: ForgejoEnsureWebhookInput,
    ) -> Result<(), ActivityError> {
        forgejo_ensure_webhook(ctx, input).await
    }

    #[activity]
    pub async fn forgejo_ensure_collaborator(
        ctx: ActivityContext,
        input: ForgejoEnsureCollaboratorInput,
    ) -> Result<(), ActivityError> {
        forgejo_ensure_collaborator(ctx, input).await
    }

    #[activity]
    pub async fn forgejo_ensure_gitops_repo_seeded(
        ctx: ActivityContext,
        input: ForgejoEnsureGitopsRepoSeededInput,
    ) -> Result<(), ActivityError> {
        forgejo_ensure_gitops_repo_seeded(ctx, input).await
    }
}
