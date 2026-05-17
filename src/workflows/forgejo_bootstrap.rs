use std::time::Duration;

use crate::activities::{
    ForgejoEnsureCollaboratorInput, ForgejoEnsureGitopsRepoSeededInput, ForgejoEnsureRepoInput,
    ForgejoEnsureUserInput, ForgejoEnsureWebhookInput,
};
use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use temporal_sdk::{ActivityOptions, WfContext, WfExitValue, WorkflowResult};
use temporal_sdk_core_protos::coresdk::AsJsonPayloadExt;
use tracing::info;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgejoBootstrapInput {
    pub forgejo_url: String,
    pub bot_username: String,
    pub bot_email: String,
    pub webhook_url: String,
    pub legacy_webhook_url: String,
    pub webhook_repos: Vec<String>,
    pub gitops_repo: String,
    pub gitops_source_url: String,
    pub gitops_revision: String,
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
            webhook_repos: std::env::var("NETAMOS_WEBHOOK_REPOS")
                .unwrap_or_else(|_| "khuedoan/blog".to_string())
                .split(',')
                .map(str::trim)
                .filter(|repo| !repo.is_empty())
                .map(ToString::to_string)
                .collect(),
            gitops_repo: std::env::var("GITOPS_REPO")
                .unwrap_or_else(|_| "khuedoan/cloudlab".to_string()),
            gitops_source_url: std::env::var("GITOPS_SOURCE_URL")
                .unwrap_or_else(|_| "https://github.com/khuedoan/cloudlab".to_string()),
            gitops_revision: std::env::var("GITOPS_REVISION")
                .unwrap_or_else(|_| "master".to_string()),
        }
    }
}

pub fn name() -> String {
    "forgejo_bootstrap".to_string()
}

pub async fn definition(ctx: WfContext) -> WorkflowResult<()> {
    let input: ForgejoBootstrapInput = serde_json::from_slice(
        &ctx.get_args()
            .first()
            .ok_or(anyhow!("missing workflow input"))?
            .data,
    )?;
    info!("starting Forgejo bootstrap: {input:?}");

    ctx.activity(ActivityOptions {
        activity_type: "forgejo_wait".to_string(),
        input: input.forgejo_url.as_json_payload()?,
        start_to_close_timeout: Some(Duration::from_secs(300)),
        ..Default::default()
    })
    .await
    .success_payload_or_error()?;

    ctx.activity(ActivityOptions {
        activity_type: "forgejo_ensure_user".to_string(),
        input: ForgejoEnsureUserInput {
            forgejo_url: input.forgejo_url.clone(),
            username: input.bot_username.clone(),
            email: input.bot_email.clone(),
        }
        .as_json_payload()?,
        start_to_close_timeout: Some(Duration::from_secs(60)),
        ..Default::default()
    })
    .await
    .success_payload_or_error()?;

    ctx.activity(ActivityOptions {
        activity_type: "forgejo_ensure_repo".to_string(),
        input: ForgejoEnsureRepoInput {
            forgejo_url: input.forgejo_url.clone(),
            repo: input.gitops_repo.clone(),
            private: false,
        }
        .as_json_payload()?,
        start_to_close_timeout: Some(Duration::from_secs(60)),
        ..Default::default()
    })
    .await
    .success_payload_or_error()?;

    ctx.activity(ActivityOptions {
        activity_type: "forgejo_ensure_collaborator".to_string(),
        input: ForgejoEnsureCollaboratorInput {
            forgejo_url: input.forgejo_url.clone(),
            repo: input.gitops_repo.clone(),
            username: input.bot_username.clone(),
            permission: "write".to_string(),
        }
        .as_json_payload()?,
        start_to_close_timeout: Some(Duration::from_secs(60)),
        ..Default::default()
    })
    .await
    .success_payload_or_error()?;

    ctx.activity(ActivityOptions {
        activity_type: "forgejo_ensure_gitops_repo_seeded".to_string(),
        input: ForgejoEnsureGitopsRepoSeededInput {
            forgejo_url: input.forgejo_url.clone(),
            repo: input.gitops_repo.clone(),
            source_url: input.gitops_source_url.clone(),
            revision: input.gitops_revision.clone(),
        }
        .as_json_payload()?,
        start_to_close_timeout: Some(Duration::from_secs(600)),
        ..Default::default()
    })
    .await
    .success_payload_or_error()?;

    for repo in input.webhook_repos {
        ctx.activity(ActivityOptions {
            activity_type: "forgejo_ensure_webhook".to_string(),
            input: ForgejoEnsureWebhookInput {
                forgejo_url: input.forgejo_url.clone(),
                repo,
                webhook_url: input.webhook_url.clone(),
                legacy_webhook_url: input.legacy_webhook_url.clone(),
            }
            .as_json_payload()?,
            start_to_close_timeout: Some(Duration::from_secs(60)),
            ..Default::default()
        })
        .await
        .success_payload_or_error()?;
    }

    Ok(WfExitValue::Normal(()))
}
