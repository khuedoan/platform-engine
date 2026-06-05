use super::{
    forgejo::ForgejoCommitStatusTarget,
    git_auth::git_command_for_url,
    process::{run_checked_command, run_stdout_command},
    workspace::TempWorkspace,
};
use crate::{
    api::{
        CreateAppRequest, CreateDeployment, CreateHttpRoute, CreatePostgres, CreateService,
        CreateVolume, KeyValue,
    },
    core::app::image::Image,
};
use anyhow::{Context, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use serde_yaml::Value as YamlValue;
use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use temporalio_sdk::activities::{ActivityContext, ActivityError};
use tokio::{fs::remove_dir_all, process::Command};
use tracing::info;

const APPS_REPOSITORY: &str = "apps";
const APPS_TAG: &str = "latest";
const FLUX_NAMESPACE: &str = "flux-system";
const SOURCE_INTERVAL: &str = "30s";
const RECONCILE_INTERVAL: &str = "1m";
const NAMESPACE_KIND: &str = "Namespace";
const SOURCE_IMAGE_REPOSITORY: &str = "apps";

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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AppTarget {
    pub tenant: String,
    pub project: String,
    pub environment: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct AppSourceTarget {
    pub source_repo: String,
    pub target: AppTarget,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppInventory {
    pub tenant: String,
    pub project: String,
    pub environment: String,
    pub resources: Vec<String>,
    pub hostnames: Vec<String>,
    pub images: Vec<String>,
    pub source_repos: Vec<String>,
}

pub async fn enqueue_gitops_publish(
    ctx: ActivityContext,
    input: EnqueueGitopsPublishInput,
) -> Result<(), ActivityError> {
    if ctx.is_cancelled() {
        return Err(ActivityError::cancelled());
    }

    ctx.record_heartbeat(vec![]);
    let client = crate::temporal::get_client()
        .await
        .map_err(anyhow::Error::from)?;
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

    let targets = scan_app_source_targets(&workspace.path().join("apps"), &input.registry)
        .map_err(ActivityError::from)?
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

    let repos = scan_app_source_targets(&workspace.path().join("apps"), &input.registry)
        .map_err(ActivityError::from)?
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
        .map_err(|error| ActivityError::from(anyhow!(error)))?;

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
            return Err(ActivityError::from(anyhow!(
                "apps/{app_path} already exists; pass force to replace it"
            )));
        }
        fs::remove_dir_all(&app_dir).map_err(|error| ActivityError::from(anyhow!(error)))?;
    }
    fs::create_dir_all(&app_dir).map_err(|error| ActivityError::from(anyhow!(error)))?;
    write_create_app_manifests(&app_dir, &input.request, &input.registry)
        .map_err(ActivityError::from)?;

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
        remove_dir_all(workspace).await.map_err(|e| anyhow!(e))?;
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppImageUpdate {
    pub repository: String,
    pub tag: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateAppVersionInput {
    pub apps_dir: String,
    pub environment: String,
    pub new_images: Vec<AppImageUpdate>,
}

pub async fn update_app_version_inner(input: UpdateAppVersionInput) -> anyhow::Result<bool> {
    let apps_dir = Path::new(&input.apps_dir);
    let mut changed = false;

    for (_tenant, tenant_dir) in child_dirs(apps_dir)? {
        for (_project, project_dir) in child_dirs(&tenant_dir)? {
            let app_dir = project_dir.join(&input.environment);
            if !app_dir.is_dir() {
                continue;
            }

            for entry in fs::read_dir(&app_dir)? {
                let path = entry?.path();
                if !is_yaml_file(&path) || is_kustomization(&path) {
                    continue;
                }

                let mut doc = read_app_manifest(&path)?;
                let mut file_changed = false;
                update_image_tags_recursive(&mut doc, &input.new_images, &mut file_changed);

                if file_changed {
                    write_yaml_manifest(&path, &doc)?;
                    changed = true;
                }
            }
        }
    }

    Ok(changed)
}

fn is_yaml_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension, "yaml" | "yml"))
}

#[derive(Debug)]
struct AppsBundle {
    apps: Vec<AppArtifact>,
    count: usize,
    root_dir: PathBuf,
}

#[derive(Debug, Clone)]
struct AppArtifact {
    dir: PathBuf,
    name: String,
    repository: String,
}

fn write_apps_bundle(
    output_dir: &Path,
    source_dir: &Path,
    repository: &str,
    tag: &str,
    registry: &str,
) -> anyhow::Result<AppsBundle> {
    fs::create_dir_all(output_dir)?;

    let mut apps = Vec::new();
    let mut count = 0;
    for (tenant, tenant_dir) in child_dirs(source_dir)? {
        for (project, project_dir) in child_dirs(&tenant_dir)? {
            for (environment, environment_dir) in child_dirs(&project_dir)? {
                let app_env = format!("{tenant}/{project}/{environment}");
                let app = app_artifact(output_dir, repository, &app_env);
                let manifest_count = copy_app_manifests(&environment_dir, &app.dir, &app.name)?;
                if manifest_count > 0 {
                    write_namespace(&app.dir, &app.name)?;
                    count += manifest_count;
                    apps.push(app);
                }
            }
        }
    }

    if count == 0 {
        return Err(anyhow!(
            "no app manifests found in {}",
            source_dir.display()
        ));
    }

    let root_dir = output_dir.join("root");
    write_root_bundle(&root_dir, &apps, tag, registry)?;

    Ok(AppsBundle {
        apps,
        count,
        root_dir,
    })
}

fn child_dirs(path: &Path) -> anyhow::Result<Vec<(String, PathBuf)>> {
    let mut dirs = Vec::new();
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|name| anyhow!("{} is not valid UTF-8", name.to_string_lossy()))?;
        dirs.push((name, entry.path()));
    }
    dirs.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(dirs)
}

fn copy_app_manifests(
    source_dir: &Path,
    output_dir: &Path,
    namespace: &str,
) -> anyhow::Result<usize> {
    let mut count = 0;
    for entry in fs::read_dir(source_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            return Err(anyhow!(
                "{}: nested app manifest directories are not supported",
                path.display()
            ));
        }
        if !is_yaml_file(&path) {
            continue;
        }
        if is_kustomization(&path) {
            return Err(anyhow!(
                "{} is not supported; put plain Kubernetes YAML in apps instead",
                path.display()
            ));
        }

        let mut manifest = read_app_manifest(&path)?;
        validate_app_manifest(&path, &manifest)?;
        set_manifest_namespace(&mut manifest, namespace)?;
        write_yaml_manifest(&output_dir.join(entry.file_name()), &manifest)?;
        count += 1;
    }

    Ok(count)
}

pub fn scan_app_source_targets(
    apps_dir: &Path,
    registry: &str,
) -> anyhow::Result<Vec<AppSourceTarget>> {
    let mut mappings = BTreeSet::new();

    for (tenant, tenant_dir) in child_dirs(apps_dir)? {
        for (project, project_dir) in child_dirs(&tenant_dir)? {
            for (environment, environment_dir) in child_dirs(&project_dir)? {
                for entry in fs::read_dir(&environment_dir)? {
                    let path = entry?.path();
                    if !is_yaml_file(&path) || is_kustomization(&path) {
                        continue;
                    }

                    let manifest = read_app_manifest(&path)?;
                    let mut image_refs = Vec::new();
                    collect_image_references(&manifest, &mut image_refs);
                    for image in image_refs {
                        if let Some(source_repo) = source_repo_from_image(registry, image) {
                            mappings.insert(AppSourceTarget {
                                source_repo,
                                target: AppTarget {
                                    tenant: tenant.clone(),
                                    project: project.clone(),
                                    environment: environment.clone(),
                                },
                            });
                        }
                    }
                }
            }
        }
    }

    Ok(mappings.into_iter().collect())
}

fn read_app_manifest(path: &Path) -> anyhow::Result<YamlValue> {
    let mut manifest = None;
    for document in serde_yaml::Deserializer::from_reader(fs::File::open(path)?) {
        let value = YamlValue::deserialize(document)?;
        if is_empty_yaml_document(&value) {
            continue;
        }
        if manifest.replace(value).is_some() {
            return Err(anyhow!(
                "{}: expected one Kubernetes manifest per file",
                path.display()
            ));
        }
    }

    manifest.ok_or_else(|| anyhow!("{}: no Kubernetes manifests found", path.display()))
}

fn validate_app_manifest(path: &Path, manifest: &YamlValue) -> anyhow::Result<()> {
    let YamlValue::Mapping(root) = manifest else {
        return Err(anyhow!(
            "{}: app manifest must be a YAML mapping",
            path.display()
        ));
    };

    required_string(root, "apiVersion")
        .ok_or_else(|| anyhow!("{}: manifest is missing apiVersion", path.display()))?;
    let kind = required_string(root, "kind")
        .ok_or_else(|| anyhow!("{}: manifest is missing kind", path.display()))?;
    let metadata = required_mapping(root, "metadata")
        .ok_or_else(|| anyhow!("{}: {kind} is missing metadata.name", path.display()))?;
    let name = required_string(metadata, "name")
        .ok_or_else(|| anyhow!("{}: {kind} is missing metadata.name", path.display()))?;

    if kind == NAMESPACE_KIND {
        return Err(anyhow!(
            "{}: Namespace manifests are generated by platform-engine",
            path.display()
        ));
    }
    if metadata.contains_key(&YamlValue::String("namespace".to_string())) {
        return Err(anyhow!(
            "{}: {kind}/{name} metadata.namespace must be omitted; platform-engine sets it from the app path",
            path.display()
        ));
    }

    let expected_filename = format!("{}-{name}.yaml", kind.to_lowercase());
    if path.file_name().and_then(|name| name.to_str()) != Some(expected_filename.as_str()) {
        return Err(anyhow!(
            "{}: expected filename {expected_filename}",
            path.display()
        ));
    }

    Ok(())
}

fn collect_image_references<'a>(node: &'a YamlValue, images: &mut Vec<&'a str>) {
    match node {
        YamlValue::Mapping(map) => {
            let image_key = YamlValue::String("image".to_string());
            if let Some(YamlValue::String(image)) = map.get(&image_key) {
                images.push(image);
            }

            for value in map.values() {
                collect_image_references(value, images);
            }
        }
        YamlValue::Sequence(seq) => {
            for value in seq {
                collect_image_references(value, images);
            }
        }
        _ => {}
    }
}

fn source_repo_from_image(registry: &str, image: &str) -> Option<String> {
    let prefix = format!(
        "{}/{SOURCE_IMAGE_REPOSITORY}/",
        registry.trim_end_matches('/')
    );
    let image = image.strip_prefix(&prefix)?;
    let repository = image_repository_path(image);
    let mut parts = repository.split('/');
    let owner = parts.next().filter(|part| !part.is_empty())?;
    let repo = parts.next().filter(|part| !part.is_empty())?;
    if parts.next().is_some() {
        return None;
    }

    Some(format!("{owner}/{repo}"))
}

fn image_repository_path(image: &str) -> &str {
    let image = image
        .split_once('@')
        .map_or(image, |(repository, _digest)| repository);
    image
        .split_once(':')
        .map_or(image, |(repository, _tag)| repository)
}

pub fn scan_app_inventory(apps_dir: &Path, registry: &str) -> anyhow::Result<Vec<AppInventory>> {
    let mut inventory = Vec::new();

    for (tenant, tenant_dir) in child_dirs(apps_dir)? {
        for (project, project_dir) in child_dirs(&tenant_dir)? {
            for (environment, environment_dir) in child_dirs(&project_dir)? {
                let mut resources = BTreeSet::new();
                let mut hostnames = BTreeSet::new();
                let mut images = BTreeSet::new();
                let mut source_repos = BTreeSet::new();

                for entry in fs::read_dir(&environment_dir)? {
                    let path = entry?.path();
                    if !is_yaml_file(&path) || is_kustomization(&path) {
                        continue;
                    }

                    let manifest = read_app_manifest(&path)?;
                    if let Some(resource) = resource_ref(&manifest) {
                        resources.insert(resource);
                    }
                    for hostname in http_route_hostnames(&manifest) {
                        hostnames.insert(hostname);
                    }

                    let mut image_refs = Vec::new();
                    collect_image_references(&manifest, &mut image_refs);
                    for image in image_refs {
                        images.insert(image.to_string());
                        if let Some(source_repo) = source_repo_from_image(registry, image) {
                            source_repos.insert(source_repo);
                        }
                    }
                }

                if !resources.is_empty() {
                    inventory.push(AppInventory {
                        tenant: tenant.clone(),
                        project: project.clone(),
                        environment: environment.clone(),
                        resources: resources.into_iter().collect(),
                        hostnames: hostnames.into_iter().collect(),
                        images: images.into_iter().collect(),
                        source_repos: source_repos.into_iter().collect(),
                    });
                }
            }
        }
    }

    Ok(inventory)
}

fn resource_ref(manifest: &YamlValue) -> Option<String> {
    let YamlValue::Mapping(root) = manifest else {
        return None;
    };
    let kind = required_string(root, "kind")?;
    let metadata = required_mapping(root, "metadata")?;
    let name = required_string(metadata, "name")?;
    Some(format!("{kind}/{name}"))
}

fn http_route_hostnames(manifest: &YamlValue) -> Vec<String> {
    let YamlValue::Mapping(root) = manifest else {
        return Vec::new();
    };
    if required_string(root, "kind") != Some("HTTPRoute") {
        return Vec::new();
    }

    let Some(spec) = required_mapping(root, "spec") else {
        return Vec::new();
    };
    match spec.get(&YamlValue::String("hostnames".to_string())) {
        Some(YamlValue::Sequence(hostnames)) => hostnames
            .iter()
            .filter_map(|value| match value {
                YamlValue::String(hostname) => Some(hostname.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn write_create_app_manifests(
    app_dir: &Path,
    request: &CreateAppRequest,
    registry: &str,
) -> anyhow::Result<usize> {
    let mut count = 0;

    if let Some(deployment) = &request.deployment {
        write_json_manifest(
            &app_dir.join(format!("deployment-{}.yaml", request.project)),
            deployment_manifest(request, deployment, registry)?,
        )?;
        count += 1;
    }
    if let Some(service) = &request.service {
        write_json_manifest(
            &app_dir.join(format!("service-{}.yaml", request.project)),
            service_manifest(request, service),
        )?;
        count += 1;
    }
    if let Some(route) = &request.http_route {
        write_json_manifest(
            &app_dir.join(format!("httproute-{}.yaml", request.project)),
            http_route_manifest(request, route),
        )?;
        count += 1;
    }
    if !request.config.is_empty() {
        write_json_manifest(
            &app_dir.join(format!("configmap-{}.yaml", request.project)),
            config_map_manifest(request, &request.config),
        )?;
        count += 1;
    }
    if !request.secrets.is_empty() {
        write_json_manifest(
            &app_dir.join(format!("secret-{}.yaml", request.project)),
            secret_manifest(request, &request.secrets),
        )?;
        count += 1;
    }
    for volume in &request.volumes {
        write_json_manifest(
            &app_dir.join(format!("persistentvolumeclaim-{}.yaml", volume.name)),
            pvc_manifest(volume),
        )?;
        count += 1;
    }
    if let Some(postgres) = &request.postgres {
        write_json_manifest(
            &app_dir.join(format!("cluster-{}-postgres.yaml", request.project)),
            postgres_manifest(request, postgres),
        )?;
        count += 1;
    }

    Ok(count)
}

fn deployment_manifest(
    request: &CreateAppRequest,
    deployment: &CreateDeployment,
    registry: &str,
) -> anyhow::Result<JsonValue> {
    let image = deployment
        .image
        .clone()
        .or_else(|| {
            deployment
                .source_repo
                .as_ref()
                .map(|repo| format!("{}/apps/{repo}:latest", registry.trim_end_matches('/')))
        })
        .context("deployment needs either an image or a source repo")?;
    let label = json!({ "app.kubernetes.io/name": &request.project });
    let mut container = json!({
        "name": &request.project,
        "image": image,
    });
    if let Some(port) = deployment.port {
        container["ports"] = json!([{ "containerPort": port, "name": "http" }]);
    }
    if !request.config.is_empty() || !request.secrets.is_empty() {
        let mut env_from = Vec::new();
        if !request.config.is_empty() {
            env_from.push(json!({ "configMapRef": { "name": &request.project } }));
        }
        if !request.secrets.is_empty() {
            env_from.push(json!({ "secretRef": { "name": &request.project } }));
        }
        container["envFrom"] = json!(env_from);
    }
    if !request.volumes.is_empty() {
        container["volumeMounts"] = json!(
            request
                .volumes
                .iter()
                .map(|volume| json!({ "name": &volume.name, "mountPath": &volume.mount_path }))
                .collect::<Vec<_>>()
        );
    }

    let mut manifest = json!({
        "apiVersion": "apps/v1",
        "kind": "Deployment",
        "metadata": { "name": &request.project },
        "spec": {
            "replicas": deployment.replicas,
            "selector": { "matchLabels": label },
            "template": {
                "metadata": {
                    "labels": {
                        "app.kubernetes.io/name": &request.project,
                        "istio.io/dataplane-mode": "ambient",
                    },
                },
                "spec": { "containers": [container] },
            },
        },
    });
    if !request.volumes.is_empty() {
        manifest["spec"]["template"]["spec"]["volumes"] = json!(
            request
                .volumes
                .iter()
                .map(|volume| json!({
                    "name": &volume.name,
                    "persistentVolumeClaim": { "claimName": &volume.name },
                }))
                .collect::<Vec<_>>()
        );
    }

    Ok(manifest)
}

fn service_manifest(request: &CreateAppRequest, service: &CreateService) -> JsonValue {
    json!({
        "apiVersion": "v1",
        "kind": "Service",
        "metadata": { "name": &request.project },
        "spec": {
            "ports": [{
                "name": "http",
                "port": service.port,
                "targetPort": "http",
            }],
            "selector": { "app.kubernetes.io/name": &request.project },
        },
    })
}

fn http_route_manifest(request: &CreateAppRequest, route: &CreateHttpRoute) -> JsonValue {
    json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "HTTPRoute",
        "metadata": { "name": &request.project },
        "spec": {
            "hostnames": [&route.hostname],
            "parentRefs": [{
                "group": "gateway.networking.k8s.io",
                "kind": "Gateway",
                "name": "gateway",
                "namespace": "istio-system",
            }],
            "rules": [{
                "backendRefs": [{ "name": &request.project, "port": route.port }],
                "matches": [{ "path": { "type": "PathPrefix", "value": "/" } }],
            }],
        },
    })
}

fn config_map_manifest(request: &CreateAppRequest, values: &[KeyValue]) -> JsonValue {
    json!({
        "apiVersion": "v1",
        "kind": "ConfigMap",
        "metadata": { "name": &request.project },
        "data": key_values(values),
    })
}

fn secret_manifest(request: &CreateAppRequest, values: &[KeyValue]) -> JsonValue {
    json!({
        "apiVersion": "v1",
        "kind": "Secret",
        "metadata": {
            "name": &request.project,
            "annotations": {
                "vault.security.banzaicloud.io/vault-addr": "http://vault.vault.svc.cluster.local:8200",
                "vault.security.banzaicloud.io/vault-path": "kubernetes",
                "vault.security.banzaicloud.io/vault-role": "default",
            },
        },
        "type": "Opaque",
        "stringData": key_values(values),
    })
}

fn pvc_manifest(volume: &CreateVolume) -> JsonValue {
    json!({
        "apiVersion": "v1",
        "kind": "PersistentVolumeClaim",
        "metadata": { "name": &volume.name },
        "spec": {
            "accessModes": ["ReadWriteOnce"],
            "resources": { "requests": { "storage": &volume.size } },
        },
    })
}

fn postgres_manifest(request: &CreateAppRequest, postgres: &CreatePostgres) -> JsonValue {
    let cluster = format!("{}-postgres", request.project);
    json!({
        "apiVersion": "postgresql.cnpg.io/v1",
        "kind": "Cluster",
        "metadata": { "name": &cluster },
        "spec": {
            "instances": 1,
            "bootstrap": {
                "initdb": {
                    "database": &request.project,
                    "owner": &request.project,
                    "secret": { "name": format!("{cluster}-app") },
                },
            },
            "storage": { "size": &postgres.size },
        },
    })
}

fn key_values(values: &[KeyValue]) -> serde_json::Map<String, JsonValue> {
    values
        .iter()
        .map(|value| (value.key.clone(), JsonValue::String(value.value.clone())))
        .collect()
}

fn write_json_manifest(path: &Path, value: JsonValue) -> anyhow::Result<()> {
    let manifest = serde_yaml::to_value(value)?;
    validate_app_manifest(path, &manifest)?;
    write_yaml_manifest(path, &manifest)
}

fn is_empty_yaml_document(value: &YamlValue) -> bool {
    matches!(value, YamlValue::Null) || matches!(value, YamlValue::Mapping(map) if map.is_empty())
}

fn required_mapping<'a>(
    map: &'a serde_yaml::Mapping,
    key: &str,
) -> Option<&'a serde_yaml::Mapping> {
    match map.get(&YamlValue::String(key.to_string())) {
        Some(YamlValue::Mapping(value)) => Some(value),
        _ => None,
    }
}

fn required_string<'a>(map: &'a serde_yaml::Mapping, key: &str) -> Option<&'a str> {
    match map.get(&YamlValue::String(key.to_string())) {
        Some(YamlValue::String(value)) if !value.is_empty() => Some(value),
        _ => None,
    }
}

fn set_manifest_namespace(manifest: &mut YamlValue, namespace: &str) -> anyhow::Result<()> {
    let YamlValue::Mapping(root) = manifest else {
        return Err(anyhow!("app manifest must be a YAML mapping"));
    };

    let metadata_key = YamlValue::String("metadata".to_string());
    let metadata = root
        .entry(metadata_key)
        .or_insert_with(|| YamlValue::Mapping(Default::default()));
    let YamlValue::Mapping(metadata) = metadata else {
        return Err(anyhow!("app manifest metadata must be a YAML mapping"));
    };

    metadata.insert(
        YamlValue::String("namespace".to_string()),
        YamlValue::String(namespace.to_string()),
    );
    Ok(())
}

fn write_yaml_manifest(path: &Path, manifest: &YamlValue) -> anyhow::Result<()> {
    fs::create_dir_all(
        path.parent()
            .ok_or_else(|| anyhow!("{} has no parent directory", path.display()))?,
    )?;
    let writer = fs::File::create(path)?;
    let mut serializer = serde_yaml::Serializer::new(writer);
    manifest.serialize(&mut serializer)?;
    Ok(())
}

fn app_artifact(output_dir: &Path, repository: &str, app_env: &str) -> AppArtifact {
    let name = app_env.replace('/', "-");
    AppArtifact {
        dir: output_dir.join("apps").join(app_env),
        name,
        repository: format!("{repository}/{app_env}"),
    }
}

fn write_namespace(output_dir: &Path, name: &str) -> anyhow::Result<()> {
    write_file(
        &output_dir.join("namespace.yaml"),
        &format!(
            r#"apiVersion: v1
kind: Namespace
metadata:
  name: {name}
  labels:
    istio.io/dataplane-mode: ambient
"#
        ),
    )
}

fn write_root_bundle(
    output_dir: &Path,
    apps: &[AppArtifact],
    tag: &str,
    registry: &str,
) -> anyhow::Result<()> {
    fs::create_dir_all(output_dir)?;

    for app in apps {
        write_file(
            &output_dir.join(format!("ocirepository-{}.yaml", app.name)),
            &format!(
                r#"apiVersion: source.toolkit.fluxcd.io/v1
kind: OCIRepository
metadata:
  name: {name}
  namespace: {namespace}
spec:
  interval: {source_interval}
  url: oci://{registry}/{repository}
  insecure: true
  ref:
    tag: {tag}
"#,
                name = app.name,
                namespace = FLUX_NAMESPACE,
                source_interval = SOURCE_INTERVAL,
                registry = registry,
                repository = app.repository,
                tag = tag,
            ),
        )?;

        write_file(
            &output_dir.join(format!("kustomization-{}.yaml", app.name)),
            &format!(
                r#"apiVersion: kustomize.toolkit.fluxcd.io/v1
kind: Kustomization
metadata:
  name: {name}
  namespace: {namespace}
spec:
  interval: {reconcile_interval}
  dependsOn:
    - name: platform
  path: .
  prune: true
  targetNamespace: {name}
  sourceRef:
    kind: OCIRepository
    name: {name}
"#,
                name = app.name,
                namespace = FLUX_NAMESPACE,
                reconcile_interval = RECONCILE_INTERVAL,
            ),
        )?;
    }

    Ok(())
}

fn write_file(path: &Path, content: &str) -> anyhow::Result<()> {
    fs::create_dir_all(
        path.parent()
            .ok_or_else(|| anyhow!("{} has no parent directory", path.display()))?,
    )?;
    fs::write(path, content)?;
    Ok(())
}

fn is_kustomization(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| matches!(name, "kustomization.yaml" | "kustomization.yml"))
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

fn update_image_tags_recursive(
    node: &mut YamlValue,
    new_images: &[AppImageUpdate],
    changed: &mut bool,
) {
    match node {
        YamlValue::Mapping(map) => {
            let image_key = YamlValue::String("image".to_string());
            if let Some(YamlValue::String(image)) = map.get_mut(&image_key) {
                for img in new_images {
                    if let Some(updated) = update_image_reference(image, img) {
                        *image = updated;
                        *changed = true;
                    }
                }
            }

            let keys: Vec<YamlValue> = map.keys().cloned().collect();
            for key in keys {
                if let Some(val) = map.get_mut(&key) {
                    update_image_tags_recursive(val, new_images, changed);
                }
            }
        }
        YamlValue::Sequence(seq) => {
            for item in seq.iter_mut() {
                update_image_tags_recursive(item, new_images, changed);
            }
        }
        _ => {}
    }
}

fn update_image_reference(current: &str, image: &AppImageUpdate) -> Option<String> {
    let tag_prefix = format!("{}:", image.repository);
    let digest_prefix = format!("{}@", image.repository);

    if current.starts_with(&tag_prefix) || current.starts_with(&digest_prefix) {
        let updated = format!("{}:{}", image.repository, image.tag);
        if current != updated {
            return Some(updated);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn write_app_fixture(root: &Path, image: &str) {
        let app_dir = root.join("khuedoan").join("blog").join("production");
        fs::create_dir_all(&app_dir).unwrap();
        fs::write(
            app_dir.join("deployment-blog.yaml"),
            format!(
                r#"apiVersion: apps/v1
kind: Deployment
metadata:
  name: blog
spec:
  template:
    spec:
      containers:
        - name: blog
          image: {image}
"#
            ),
        )
        .unwrap();
        fs::write(
            app_dir.join("service-blog.yaml"),
            r#"apiVersion: v1
kind: Service
metadata:
  name: blog
"#,
        )
        .unwrap();
    }

    #[tokio::test]
    async fn test_update_app_version_changes() {
        let tmp = PathBuf::from("/tmp/test-cloudlab-apps-1");
        let _ = tokio::fs::remove_dir_all(&tmp).await;
        tokio::fs::create_dir_all(&tmp).await.unwrap();
        write_app_fixture(
            &tmp,
            "registry.registry.svc.cluster.local/apps/khuedoan/blog:old-tag",
        );

        let changed = update_app_version_inner(UpdateAppVersionInput {
            apps_dir: tmp.to_string_lossy().to_string(),
            environment: "production".to_string(),
            new_images: vec![AppImageUpdate {
                repository: "registry.registry.svc.cluster.local/apps/khuedoan/blog".to_string(),
                tag: "test-tag-123".to_string(),
            }],
        })
        .await
        .unwrap();

        assert!(changed);
        let deployment =
            fs::read_to_string(tmp.join("khuedoan/blog/production/deployment-blog.yaml")).unwrap();
        assert!(deployment.contains(
            "image: registry.registry.svc.cluster.local/apps/khuedoan/blog:test-tag-123"
        ));
    }

    #[tokio::test]
    async fn test_update_app_version_no_changes() {
        let tmp = PathBuf::from("/tmp/test-cloudlab-apps-2");
        let _ = tokio::fs::remove_dir_all(&tmp).await;
        tokio::fs::create_dir_all(&tmp).await.unwrap();
        write_app_fixture(
            &tmp,
            "registry.registry.svc.cluster.local/apps/khuedoan/blog:6fbd90b77a81e0bcb330fddaa230feff744a7010",
        );

        let changed = update_app_version_inner(UpdateAppVersionInput {
            apps_dir: tmp.to_string_lossy().to_string(),
            environment: "production".to_string(),
            new_images: vec![AppImageUpdate {
                repository: "registry.registry.svc.cluster.local/apps/khuedoan/blog".to_string(),
                tag: "6fbd90b77a81e0bcb330fddaa230feff744a7010".to_string(),
            }],
        })
        .await
        .unwrap();

        assert!(!changed);
    }

    #[test]
    fn test_source_repo_from_image() {
        let registry = "registry.registry.svc.cluster.local";

        assert_eq!(
            source_repo_from_image(
                registry,
                "registry.registry.svc.cluster.local/apps/khuedoan/blog:abc123"
            ),
            Some("khuedoan/blog".to_string())
        );
        assert_eq!(
            source_repo_from_image(
                registry,
                "registry.registry.svc.cluster.local/apps/khuedoan/blog@sha256:abc123"
            ),
            Some("khuedoan/blog".to_string())
        );
        assert_eq!(
            source_repo_from_image(
                registry,
                "registry.registry.svc.cluster.local/vendor/blog:1"
            ),
            None
        );
        assert_eq!(
            source_repo_from_image(
                registry,
                "registry.registry.svc.cluster.local/apps/khuedoan/team/blog:1"
            ),
            None
        );
        assert_eq!(
            source_repo_from_image(registry, "ghcr.io/khuedoan/blog:1"),
            None
        );
    }

    #[test]
    fn test_scan_app_source_targets_allows_multiple_sources() {
        let source = PathBuf::from("/tmp/test-cloudlab-app-source-targets");
        let _ = fs::remove_dir_all(&source);
        let app_dir = source.join("khuedoan").join("blog").join("production");
        fs::create_dir_all(&app_dir).unwrap();
        fs::write(
            app_dir.join("deployment-blog.yaml"),
            r#"apiVersion: apps/v1
kind: Deployment
metadata:
  name: blog
spec:
  template:
    spec:
      containers:
        - name: api
          image: registry.registry.svc.cluster.local/apps/khuedoan/api:old
        - name: worker
          image: registry.registry.svc.cluster.local/apps/khuedoan/worker:old
        - name: sidecar
          image: ghcr.io/example/sidecar:1
"#,
        )
        .unwrap();

        let mappings =
            scan_app_source_targets(&source, "registry.registry.svc.cluster.local").unwrap();

        assert_eq!(mappings.len(), 2);
        assert!(mappings.iter().any(|mapping| {
            mapping.source_repo == "khuedoan/api"
                && mapping.target
                    == AppTarget {
                        tenant: "khuedoan".to_string(),
                        project: "blog".to_string(),
                        environment: "production".to_string(),
                    }
        }));
        assert!(
            mappings
                .iter()
                .any(|mapping| mapping.source_repo == "khuedoan/worker")
        );
    }

    #[test]
    fn test_write_apps_bundle() {
        let source = PathBuf::from("/tmp/test-cloudlab-apps-bundle-source");
        let output = PathBuf::from("/tmp/test-cloudlab-apps-bundle-output");
        let _ = fs::remove_dir_all(&source);
        let _ = fs::remove_dir_all(&output);
        write_app_fixture(&source, "docker.io/khuedoan/blog:test-tag");

        let bundle = write_apps_bundle(
            &output,
            &source,
            "apps",
            "latest",
            "registry.registry.svc.cluster.local",
        )
        .unwrap();

        assert_eq!(bundle.count, 2);
        assert_eq!(bundle.apps[0].name, "khuedoan-blog-production");
        assert!(
            output
                .join("apps/khuedoan/blog/production/namespace.yaml")
                .exists()
        );
        assert!(
            fs::read_to_string(output.join("apps/khuedoan/blog/production/service-blog.yaml"))
                .unwrap()
                .contains("namespace: khuedoan-blog-production")
        );
        assert!(
            fs::read_to_string(output.join("root/ocirepository-khuedoan-blog-production.yaml"))
                .unwrap()
                .contains(
                    "url: oci://registry.registry.svc.cluster.local/apps/khuedoan/blog/production"
                )
        );
        assert!(
            fs::read_to_string(output.join("root/kustomization-khuedoan-blog-production.yaml"))
                .unwrap()
                .contains("targetNamespace: khuedoan-blog-production")
        );
    }

    #[test]
    fn test_write_apps_bundle_rejects_invalid_manifests() {
        let cases = [
            (
                "multiple manifests",
                "service-blog.yaml",
                r#"apiVersion: v1
kind: Service
metadata:
  name: blog
---
apiVersion: v1
kind: Service
metadata:
  name: blog
"#,
                "expected one Kubernetes manifest per file",
            ),
            (
                "namespaced manifest",
                "deployment-blog.yaml",
                r#"apiVersion: apps/v1
kind: Deployment
metadata:
  name: blog
  namespace: other
"#,
                "metadata.namespace must be omitted",
            ),
            (
                "namespace manifest",
                "namespace-khuedoan-blog-production.yaml",
                r#"apiVersion: v1
kind: Namespace
metadata:
  name: khuedoan-blog-production
"#,
                "Namespace manifests are generated by platform-engine",
            ),
            (
                "wrong filename",
                "blog.yaml",
                r#"apiVersion: apps/v1
kind: Deployment
metadata:
  name: blog
"#,
                "expected filename deployment-blog.yaml",
            ),
        ];

        for (index, (name, filename, content, want_error)) in cases.into_iter().enumerate() {
            let source = PathBuf::from(format!(
                "/tmp/test-cloudlab-apps-bundle-invalid-source-{index}"
            ));
            let output = PathBuf::from(format!(
                "/tmp/test-cloudlab-apps-bundle-invalid-output-{index}"
            ));
            let _ = fs::remove_dir_all(&source);
            let _ = fs::remove_dir_all(&output);

            let app_dir = source.join("khuedoan").join("blog").join("production");
            fs::create_dir_all(&app_dir).unwrap();
            fs::write(app_dir.join(filename), content).unwrap();

            let error = write_apps_bundle(
                &output,
                &source,
                "apps",
                "latest",
                "registry.registry.svc.cluster.local",
            )
            .expect_err(name);
            assert!(
                error.to_string().contains(want_error),
                "{name}: expected {want_error:?}, got {error}"
            );
        }
    }

    #[test]
    fn test_write_create_app_manifests_and_scan_inventory() {
        let output = PathBuf::from("/tmp/test-cloudlab-create-app-manifests");
        let _ = fs::remove_dir_all(&output);
        fs::create_dir_all(&output).unwrap();

        let request = CreateAppRequest {
            tenant: "test".to_string(),
            project: "example".to_string(),
            environment: "staging".to_string(),
            force: false,
            deployment: Some(CreateDeployment {
                image: None,
                source_repo: Some("khuedoan/example-service".to_string()),
                replicas: 1,
                port: Some(3000),
            }),
            service: Some(CreateService { port: 3000 }),
            http_route: Some(CreateHttpRoute {
                hostname: "example.staging.khuedoan.com".to_string(),
                port: 3000,
            }),
            config: vec![KeyValue {
                key: "GREETING".to_string(),
                value: "hello".to_string(),
            }],
            secrets: vec![KeyValue {
                key: "TOKEN".to_string(),
                value: "vault:secret/data/example#token".to_string(),
            }],
            volumes: vec![CreateVolume {
                name: "data".to_string(),
                size: "1Gi".to_string(),
                mount_path: "/data".to_string(),
            }],
            postgres: None,
        };

        let app_dir = output.join("test/example/staging");
        fs::create_dir_all(&app_dir).unwrap();
        let count =
            write_create_app_manifests(&app_dir, &request, "registry.registry.svc.cluster.local")
                .unwrap();

        assert_eq!(count, 6);
        let deployment = fs::read_to_string(app_dir.join("deployment-example.yaml")).unwrap();
        assert!(deployment.contains(
            "image: registry.registry.svc.cluster.local/apps/khuedoan/example-service:latest"
        ));
        assert!(!deployment.contains("namespace:"));

        let inventory = scan_app_inventory(&output, "registry.registry.svc.cluster.local").unwrap();
        assert_eq!(inventory.len(), 1);
        assert_eq!(inventory[0].tenant, "test");
        assert_eq!(inventory[0].project, "example");
        assert_eq!(inventory[0].environment, "staging");
        assert!(
            inventory[0]
                .hostnames
                .contains(&"example.staging.khuedoan.com".to_string())
        );
        assert!(
            inventory[0]
                .source_repos
                .contains(&"khuedoan/example-service".to_string())
        );
    }
}
