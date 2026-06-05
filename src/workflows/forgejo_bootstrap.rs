use std::time::Duration;

use super::options::command_activity_options;
use crate::activities::{
    FindGitopsSourceReposInput, ForgejoDeleteWebhookInput, ForgejoEnsureCollaboratorInput,
    ForgejoEnsureGitopsRepoSeededInput, ForgejoEnsureRepoInput, ForgejoEnsureSystemWebhookInput,
    ForgejoEnsureUserInput, PlatformActivities,
};
use serde::{Deserialize, Serialize};
use temporalio_macros::{workflow, workflow_methods};
use temporalio_sdk::{ActivityOptions, WorkflowContext, WorkflowContextView, WorkflowResult};
use tracing::info;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgejoBootstrapInput {
    pub forgejo_url: String,
    pub bot_username: String,
    pub bot_email: String,
    pub webhook_url: String,
    pub legacy_webhook_url: String,
    pub gitops_repo: String,
    pub gitops_source_url: String,
    pub gitops_revision: String,
    pub registry: String,
}

impl ForgejoBootstrapInput {
    pub fn from_env() -> Self {
        Self {
            forgejo_url: std::env::var("FORGEJO_URL").unwrap_or_else(|_| {
                "http://forgejo-http.forgejo.svc.cluster.local:3000".to_string()
            }),
            bot_username: std::env::var("NETAMOS_USERNAME").unwrap_or_else(|_| "Bot".to_string()),
            bot_email: std::env::var("NETAMOS_EMAIL")
                .unwrap_or_else(|_| "bot@cloudlab.khuedoan.com".to_string()),
            webhook_url: std::env::var("NETAMOS_WEBHOOK_URL").unwrap_or_else(|_| {
                "http://netamos.netamos.svc.cluster.local:8080/webhooks/gitea".to_string()
            }),
            legacy_webhook_url: std::env::var("NETAMOS_WEBHOOK_LEGACY_URL")
                .unwrap_or_else(|_| "http://netamos.netamos.svc.cluster.local:8080".to_string()),
            gitops_repo: std::env::var("GITOPS_REPO")
                .unwrap_or_else(|_| "khuedoan/cloudlab".to_string()),
            gitops_source_url: std::env::var("GITOPS_SOURCE_URL")
                .unwrap_or_else(|_| "https://github.com/khuedoan/cloudlab".to_string()),
            gitops_revision: std::env::var("GITOPS_REVISION")
                .unwrap_or_else(|_| "master".to_string()),
            registry: std::env::var("REGISTRY").unwrap_or_else(|_| "localhost:5000".to_string()),
        }
    }
}

#[workflow]
pub struct ForgejoBootstrapWorkflow {
    input: ForgejoBootstrapInput,
}

#[workflow_methods]
impl ForgejoBootstrapWorkflow {
    #[init]
    fn new(_ctx: &WorkflowContextView, input: ForgejoBootstrapInput) -> Self {
        Self { input }
    }

    #[run]
    pub async fn run(ctx: &mut WorkflowContext<Self>) -> WorkflowResult<()> {
        let input = ctx.state(|state| state.input.clone());
        if !ctx.is_replaying() {
            info!("starting Forgejo bootstrap: {input:?}");
        }

        ctx.start_activity(
            PlatformActivities::forgejo_wait,
            input.forgejo_url.clone(),
            command_activity_options(Duration::from_secs(300)),
        )
        .await?;

        ctx.start_activity(
            PlatformActivities::forgejo_ensure_user,
            ForgejoEnsureUserInput {
                forgejo_url: input.forgejo_url.clone(),
                username: input.bot_username.clone(),
                email: input.bot_email.clone(),
            },
            ActivityOptions::start_to_close_timeout(Duration::from_secs(60)),
        )
        .await?;

        ctx.start_activity(
            PlatformActivities::forgejo_ensure_repo,
            ForgejoEnsureRepoInput {
                forgejo_url: input.forgejo_url.clone(),
                repo: input.gitops_repo.clone(),
                private: false,
            },
            ActivityOptions::start_to_close_timeout(Duration::from_secs(60)),
        )
        .await?;

        ctx.start_activity(
            PlatformActivities::forgejo_ensure_collaborator,
            ForgejoEnsureCollaboratorInput {
                forgejo_url: input.forgejo_url.clone(),
                repo: input.gitops_repo.clone(),
                username: input.bot_username.clone(),
                permission: "write".to_string(),
            },
            ActivityOptions::start_to_close_timeout(Duration::from_secs(60)),
        )
        .await?;

        ctx.start_activity(
            PlatformActivities::forgejo_ensure_system_webhook,
            ForgejoEnsureSystemWebhookInput {
                forgejo_url: input.forgejo_url.clone(),
                webhook_url: input.webhook_url.clone(),
                legacy_webhook_url: input.legacy_webhook_url.clone(),
            },
            ActivityOptions::start_to_close_timeout(Duration::from_secs(60)),
        )
        .await?;

        ctx.start_activity(
            PlatformActivities::forgejo_ensure_gitops_repo_seeded,
            ForgejoEnsureGitopsRepoSeededInput {
                forgejo_url: input.forgejo_url.clone(),
                repo: input.gitops_repo.clone(),
                source_url: input.gitops_source_url.clone(),
                revision: input.gitops_revision.clone(),
            },
            command_activity_options(Duration::from_secs(600)),
        )
        .await?;

        let gitops_url = format!(
            "{}/{}.git",
            input.forgejo_url.trim_end_matches('/'),
            input.gitops_repo
        );
        let source_repos = ctx
            .start_activity(
                PlatformActivities::find_gitops_source_repos,
                FindGitopsSourceReposInput {
                    url: gitops_url,
                    revision: input.gitops_revision.clone(),
                    registry: input.registry.clone(),
                },
                command_activity_options(Duration::from_secs(300)),
            )
            .await?;

        for repo in source_repos {
            ctx.start_activity(
                PlatformActivities::forgejo_delete_webhook,
                ForgejoDeleteWebhookInput {
                    forgejo_url: input.forgejo_url.clone(),
                    repo,
                    webhook_url: input.webhook_url.clone(),
                    legacy_webhook_url: input.legacy_webhook_url.clone(),
                },
                ActivityOptions::start_to_close_timeout(Duration::from_secs(60)),
            )
            .await?;
        }

        Ok(())
    }
}
