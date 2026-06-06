use super::manifest::{
    child_dirs, is_kustomization, is_namespace_manifest, is_yaml_file, read_app_manifest,
    set_manifest_namespace, validate_app_manifest, validate_app_namespace, write_file,
    write_yaml_manifest,
};
use anyhow::anyhow;
use std::{
    fs,
    path::{Path, PathBuf},
};

const FLUX_NAMESPACE: &str = "flux-system";
const SOURCE_INTERVAL: &str = "30s";
const RECONCILE_INTERVAL: &str = "1m";

#[derive(Debug)]
pub(crate) struct AppsBundle {
    pub(crate) apps: Vec<AppArtifact>,
    pub(crate) count: usize,
    pub(crate) root_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub(crate) struct AppArtifact {
    pub(crate) dir: PathBuf,
    pub(crate) name: String,
    pub(crate) repository: String,
}

pub(crate) fn write_apps_bundle(
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

fn copy_app_manifests(
    source_dir: &Path,
    output_dir: &Path,
    namespace: &str,
) -> anyhow::Result<usize> {
    let mut count = 0;
    let mut found_namespace = false;
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
        if is_namespace_manifest(&manifest) {
            validate_app_namespace(&path, &manifest, namespace)?;
            found_namespace = true;
        } else {
            set_manifest_namespace(&mut manifest, namespace)?;
        }
        write_yaml_manifest(&output_dir.join(entry.file_name()), &manifest)?;
        count += 1;
    }

    if !found_namespace {
        return Err(anyhow!(
            "{}: missing namespace.yaml for app namespace {namespace}",
            source_dir.display()
        ));
    }

    Ok(count)
}

fn app_artifact(output_dir: &Path, repository: &str, app_env: &str) -> AppArtifact {
    let name = app_env.replace('/', "-");
    AppArtifact {
        dir: output_dir.join("apps").join(app_env),
        name,
        repository: format!("{repository}/{app_env}"),
    }
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
