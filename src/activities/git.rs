use super::{
    forgejo::ForgejoCommitStatusTarget,
    git_auth::git_command_for_url,
    process::{run_checked_command, run_stdout_command},
    workspace::TempWorkspace,
};
use crate::core::app::image::Image;
use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use serde_yaml::Value as YamlValue;
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateGitopsImageInput {
    pub url: String,
    pub revision: String,
    pub tenant: String,
    pub project: String,
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
        tenant: input.tenant.clone(),
        project: input.project.clone(),
        environment: input.environment.clone(),
        new_images: vec![AppImageUpdate {
            repository,
            tag: input.image.tag.clone(),
        }],
    })
    .await?;

    let mut commit_sha = None;
    if changed {
        commit_sha = Some(commit_and_push_gitops(&ctx, workspace.path(), &input).await?);
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

async fn commit_and_push_gitops(
    ctx: &ActivityContext,
    workspace: &Path,
    input: &UpdateGitopsImageInput,
) -> Result<String, ActivityError> {
    let app_path = Path::new("apps")
        .join(&input.tenant)
        .join(&input.project)
        .join(&input.environment);
    let app_path = app_path.to_string_lossy().to_string();

    let mut command = Command::new("git");
    command.args(["add", &app_path]).current_dir(workspace);
    run_checked_command(ctx, &mut command, "git add app version").await?;

    let commit_message = format!(
        "chore({}/{}): update {} image",
        input.tenant, input.project, input.environment
    );
    let mut command = Command::new("git");
    command
        .args(["commit", "-m", &commit_message])
        .current_dir(workspace);
    run_checked_command(ctx, &mut command, "git commit app version").await?;

    let mut command = Command::new("git");
    command.args(["rev-parse", "HEAD"]).current_dir(workspace);
    let commit_sha = run_stdout_command(ctx, &mut command, "git rev-parse HEAD").await?;

    let git_username = env::var("GIT_USERNAME").unwrap_or_else(|_| "git".to_string());
    let git_password = env::var("GIT_PASSWORD").unwrap_or_else(|_| "password".to_string());
    let mut command = git_command_for_url(&input.url, &git_username, &git_password);
    let branch_ref = format!("HEAD:{}", input.revision);
    command
        .args(["push", "origin", &branch_ref])
        .current_dir(workspace);
    run_checked_command(ctx, &mut command, "git push app version").await?;

    Ok(commit_sha)
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

    let git_username = env::var("GIT_USERNAME").unwrap_or_else(|_| "git".to_string());
    let git_password = env::var("GIT_PASSWORD").unwrap_or_else(|_| "password".to_string());
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
    pub tenant: String,
    pub project: String,
    pub environment: String,
    pub new_images: Vec<AppImageUpdate>,
}

pub async fn update_app_version_inner(input: UpdateAppVersionInput) -> anyhow::Result<bool> {
    let app_dir = Path::new(&input.apps_dir)
        .join(&input.tenant)
        .join(&input.project)
        .join(&input.environment);
    let mut changed = false;

    for entry in fs::read_dir(&app_dir)? {
        let path = entry?.path();
        if !is_yaml_file(&path) {
            continue;
        }

        let mut doc: YamlValue = serde_yaml::from_reader(fs::File::open(&path)?)?;
        let mut file_changed = false;
        update_image_tags_recursive(&mut doc, &input.new_images, &mut file_changed);

        if file_changed {
            let writer = fs::File::create(&path)?;
            let mut ser = serde_yaml::Serializer::new(writer);
            doc.serialize(&mut ser)?;
            changed = true;
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

        let output_path = output_dir.join(entry.file_name());
        let mut manifest: YamlValue = serde_yaml::from_reader(fs::File::open(&path)?)?;
        set_manifest_namespace(&mut manifest, namespace)?;
        write_yaml_manifest(&output_path, &manifest)?;
        count += 1;
    }

    Ok(count)
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
        write_app_fixture(&tmp, "docker.io/khuedoan/blog:old-tag");

        let changed = update_app_version_inner(UpdateAppVersionInput {
            apps_dir: tmp.to_string_lossy().to_string(),
            tenant: "khuedoan".to_string(),
            project: "blog".to_string(),
            environment: "production".to_string(),
            new_images: vec![AppImageUpdate {
                repository: "docker.io/khuedoan/blog".to_string(),
                tag: "test-tag-123".to_string(),
            }],
        })
        .await
        .unwrap();

        assert!(changed);
        let deployment =
            fs::read_to_string(tmp.join("khuedoan/blog/production/deployment-blog.yaml")).unwrap();
        assert!(deployment.contains("image: docker.io/khuedoan/blog:test-tag-123"));
    }

    #[tokio::test]
    async fn test_update_app_version_no_changes() {
        let tmp = PathBuf::from("/tmp/test-cloudlab-apps-2");
        let _ = tokio::fs::remove_dir_all(&tmp).await;
        tokio::fs::create_dir_all(&tmp).await.unwrap();
        write_app_fixture(
            &tmp,
            "docker.io/khuedoan/blog:6fbd90b77a81e0bcb330fddaa230feff744a7010",
        );

        let changed = update_app_version_inner(UpdateAppVersionInput {
            apps_dir: tmp.to_string_lossy().to_string(),
            tenant: "khuedoan".to_string(),
            project: "blog".to_string(),
            environment: "production".to_string(),
            new_images: vec![AppImageUpdate {
                repository: "docker.io/khuedoan/blog".to_string(),
                tag: "6fbd90b77a81e0bcb330fddaa230feff744a7010".to_string(),
            }],
        })
        .await
        .unwrap();

        assert!(!changed);
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
}
