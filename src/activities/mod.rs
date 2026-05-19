mod app;
mod forgejo;
mod git;
mod git_auth;
mod process;
mod workspace;

pub use app::*;
pub use forgejo::*;
pub use git::*;

use crate::core::app::image::Image;
use temporalio_macros::activities;
use temporalio_sdk::activities::{ActivityContext, ActivityError};

#[derive(Clone, Copy)]
pub struct PlatformActivities;

#[activities]
impl PlatformActivities {
    #[activity]
    pub async fn publish_image_from_source(
        ctx: ActivityContext,
        input: PublishImageFromSourceInput,
    ) -> Result<Image, ActivityError> {
        publish_image_from_source(ctx, input).await
    }

    #[activity]
    pub async fn update_gitops_image(
        ctx: ActivityContext,
        input: UpdateGitopsImageInput,
    ) -> Result<UpdateGitopsImageResult, ActivityError> {
        update_gitops_image(ctx, input).await
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
