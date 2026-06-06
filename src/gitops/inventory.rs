use super::{
    AppInventory, AppSourceTarget, AppTarget,
    manifest::{
        child_dirs, is_kustomization, is_yaml_file, read_app_manifest, required_mapping,
        required_string,
    },
};
use serde_yaml::Value as YamlValue;
use std::{collections::BTreeSet, fs, path::Path};

const SOURCE_IMAGE_REPOSITORY: &str = "apps";

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

pub(crate) fn source_repo_from_image(registry: &str, image: &str) -> Option<String> {
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
    match spec.get(YamlValue::String("hostnames".to_string())) {
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
