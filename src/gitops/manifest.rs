use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use serde_yaml::Value as YamlValue;
use std::{
    fs,
    path::{Path, PathBuf},
};

const NAMESPACE_KIND: &str = "Namespace";
const NAMESPACE_FILENAME: &str = "namespace.yaml";

pub(crate) fn is_yaml_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension, "yaml" | "yml"))
}

pub(crate) fn child_dirs(path: &Path) -> anyhow::Result<Vec<(String, PathBuf)>> {
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

pub(crate) fn read_app_manifest(path: &Path) -> anyhow::Result<YamlValue> {
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

pub(crate) fn validate_app_manifest(path: &Path, manifest: &YamlValue) -> anyhow::Result<()> {
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
        if path.file_name().and_then(|name| name.to_str()) != Some(NAMESPACE_FILENAME) {
            return Err(anyhow!(
                "{}: expected filename namespace.yaml",
                path.display()
            ));
        }
        return Ok(());
    }

    if metadata.contains_key(YamlValue::String("namespace".to_string())) {
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

pub(crate) fn validate_app_namespace(
    path: &Path,
    manifest: &YamlValue,
    namespace: &str,
) -> anyhow::Result<()> {
    let YamlValue::Mapping(root) = manifest else {
        return Err(anyhow!(
            "{}: app manifest must be a YAML mapping",
            path.display()
        ));
    };
    let kind = required_string(root, "kind")
        .ok_or_else(|| anyhow!("{}: manifest is missing kind", path.display()))?;
    if kind != NAMESPACE_KIND {
        return Err(anyhow!(
            "{}: namespace.yaml must be a Namespace",
            path.display()
        ));
    }
    let metadata = required_mapping(root, "metadata")
        .ok_or_else(|| anyhow!("{}: Namespace is missing metadata.name", path.display()))?;
    let name = required_string(metadata, "name")
        .ok_or_else(|| anyhow!("{}: Namespace is missing metadata.name", path.display()))?;
    if name != namespace {
        return Err(anyhow!(
            "{}: Namespace name must be {namespace}",
            path.display()
        ));
    }

    Ok(())
}

pub(crate) fn is_namespace_manifest(manifest: &YamlValue) -> bool {
    let YamlValue::Mapping(root) = manifest else {
        return false;
    };
    required_string(root, "kind") == Some(NAMESPACE_KIND)
}

fn is_empty_yaml_document(value: &YamlValue) -> bool {
    matches!(value, YamlValue::Null) || matches!(value, YamlValue::Mapping(map) if map.is_empty())
}

pub(crate) fn required_mapping<'a>(
    map: &'a serde_yaml::Mapping,
    key: &str,
) -> Option<&'a serde_yaml::Mapping> {
    match map.get(YamlValue::String(key.to_string())) {
        Some(YamlValue::Mapping(value)) => Some(value),
        _ => None,
    }
}

pub(crate) fn required_string<'a>(map: &'a serde_yaml::Mapping, key: &str) -> Option<&'a str> {
    match map.get(YamlValue::String(key.to_string())) {
        Some(YamlValue::String(value)) if !value.is_empty() => Some(value),
        _ => None,
    }
}

pub(crate) fn set_manifest_namespace(
    manifest: &mut YamlValue,
    namespace: &str,
) -> anyhow::Result<()> {
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

pub(crate) fn write_yaml_manifest(path: &Path, manifest: &YamlValue) -> anyhow::Result<()> {
    fs::create_dir_all(
        path.parent()
            .ok_or_else(|| anyhow!("{} has no parent directory", path.display()))?,
    )?;
    let writer = fs::File::create(path)?;
    let mut serializer = serde_yaml::Serializer::new(writer);
    manifest.serialize(&mut serializer)?;
    Ok(())
}

pub(crate) fn write_file(path: &Path, content: &str) -> anyhow::Result<()> {
    fs::create_dir_all(
        path.parent()
            .ok_or_else(|| anyhow!("{} has no parent directory", path.display()))?,
    )?;
    fs::write(path, content)?;
    Ok(())
}

pub(crate) fn is_kustomization(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| matches!(name, "kustomization.yaml" | "kustomization.yml"))
}
