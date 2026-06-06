use super::{
    AppImageUpdate, UpdateAppVersionInput,
    manifest::{
        child_dirs, is_kustomization, is_yaml_file, read_app_manifest, write_yaml_manifest,
    },
};
use serde_yaml::Value as YamlValue;
use std::{fs, path::Path};

pub(crate) async fn update_app_version_inner(input: UpdateAppVersionInput) -> anyhow::Result<bool> {
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
