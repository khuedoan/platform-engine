use super::{
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
use std::path::Path;
use temporalio_sdk::activities::{ActivityContext, ActivityError};
use tokio::{fs::remove_dir_all, process::Command};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateGitopsImageInput {
    pub url: String,
    pub revision: String,
    pub namespace: String,
    pub app: String,
    pub cluster: String,
    pub image: Image,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateGitopsImageResult {
    pub changed: bool,
    pub commit_sha: Option<String>,
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
        namespace: input.namespace.clone(),
        app: input.app.clone(),
        cluster: input.cluster.clone(),
        new_images: vec![AppImageUpdate {
            repository,
            tag: input.image.tag.clone(),
        }],
    })
    .await?;

    if !changed {
        return Ok(UpdateGitopsImageResult {
            changed: false,
            commit_sha: None,
        });
    }

    let app_file_path = Path::new("apps")
        .join(&input.namespace)
        .join(&input.app)
        .join(format!("{}.yaml", input.cluster));
    let app_file_path = app_file_path.to_string_lossy().to_string();

    let mut command = Command::new("git");
    command
        .args(["add", &app_file_path])
        .current_dir(workspace.path());
    run_checked_command(&ctx, &mut command, "git add app version").await?;

    let commit_message = format!(
        "chore({}/{}): update {} version",
        input.namespace, input.app, input.cluster
    );
    let mut command = Command::new("git");
    command
        .args(["commit", "-m", &commit_message])
        .current_dir(workspace.path());
    run_checked_command(&ctx, &mut command, "git commit app version").await?;

    let mut command = Command::new("git");
    command
        .args(["rev-parse", "HEAD"])
        .current_dir(workspace.path());
    let commit_sha = run_stdout_command(&ctx, &mut command, "git rev-parse HEAD").await?;

    let git_username = env::var("GIT_USERNAME").unwrap_or_else(|_| "git".to_string());
    let git_password = env::var("GIT_PASSWORD").unwrap_or_else(|_| "password".to_string());
    let mut command = git_command_for_url(&input.url, &git_username, &git_password);
    let branch_ref = format!("HEAD:{}", input.revision);
    command
        .args(["push", "origin", &branch_ref])
        .current_dir(workspace.path());
    run_checked_command(&ctx, &mut command, "git push app version").await?;

    Ok(UpdateGitopsImageResult {
        changed: true,
        commit_sha: Some(commit_sha),
    })
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
    pub namespace: String,
    pub app: String,
    pub cluster: String,
    pub new_images: Vec<AppImageUpdate>,
}

pub async fn update_app_version_inner(input: UpdateAppVersionInput) -> anyhow::Result<bool> {
    let values_path = Path::new(&input.apps_dir)
        .join(&input.namespace)
        .join(&input.app)
        .join(format!("{}.yaml", input.cluster));

    let mut doc: YamlValue = serde_yaml::from_reader(fs::File::open(&values_path)?)?;

    let mut changed = false;
    update_image_tags_recursive(&mut doc, &input.new_images, &mut changed);

    if changed {
        let writer = fs::File::create(&values_path)?;
        let mut ser = serde_yaml::Serializer::new(writer);
        doc.serialize(&mut ser)?;
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
            if let Some(YamlValue::Mapping(image_map)) =
                map.get_mut(YamlValue::String("image".to_string()))
            {
                let repo_key = YamlValue::String("repository".to_string());
                let tag_key = YamlValue::String("tag".to_string());
                let (repo_opt, tag_opt) = (
                    image_map.get(&repo_key).cloned(),
                    image_map.get(&tag_key).cloned(),
                );
                if let (Some(YamlValue::String(repo_str)), Some(YamlValue::String(tag_str))) =
                    (repo_opt, tag_opt)
                {
                    for img in new_images {
                        if repo_str == img.repository && tag_str != img.tag {
                            image_map.insert(tag_key.clone(), YamlValue::String(img.tag.clone()));
                            *changed = true;
                        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn copy_dir_recursive(src: &Path, dst: &Path) {
        if !dst.exists() {
            fs::create_dir_all(dst).unwrap();
        }
        for entry in fs::read_dir(src).unwrap() {
            let entry = entry.unwrap();
            let file_type = entry.file_type().unwrap();
            let src_path = entry.path();
            let dst_path = dst.join(entry.file_name());
            if file_type.is_dir() {
                copy_dir_recursive(&src_path, &dst_path);
            } else {
                fs::copy(&src_path, &dst_path).unwrap();
            }
        }
    }

    #[tokio::test]
    async fn test_update_app_version_changes() {
        let tmp = PathBuf::from("/tmp/test-cloudlab-apps-1");
        let _ = tokio::fs::remove_dir_all(&tmp).await;
        tokio::fs::create_dir_all(&tmp).await.unwrap();
        copy_dir_recursive(Path::new("testdata/cloudlab/apps"), &tmp);

        let changed = update_app_version_inner(UpdateAppVersionInput {
            apps_dir: tmp.to_string_lossy().to_string(),
            namespace: "khuedoan".to_string(),
            app: "blog".to_string(),
            cluster: "production".to_string(),
            new_images: vec![AppImageUpdate {
                repository: "docker.io/khuedoan/blog".to_string(),
                tag: "test-tag-123".to_string(),
            }],
        })
        .await
        .unwrap();

        assert!(changed);
    }

    #[tokio::test]
    async fn test_update_app_version_no_changes() {
        let tmp = PathBuf::from("/tmp/test-cloudlab-apps-2");
        let _ = tokio::fs::remove_dir_all(&tmp).await;
        tokio::fs::create_dir_all(&tmp).await.unwrap();
        copy_dir_recursive(Path::new("testdata/cloudlab/apps"), &tmp);

        let changed = update_app_version_inner(UpdateAppVersionInput {
            apps_dir: tmp.to_string_lossy().to_string(),
            namespace: "khuedoan".to_string(),
            app: "blog".to_string(),
            cluster: "production".to_string(),
            new_images: vec![AppImageUpdate {
                repository: "docker.io/khuedoan/blog".to_string(),
                tag: "6fbd90b77a81e0bcb330fddaa230feff744a7010".to_string(),
            }],
        })
        .await
        .unwrap();

        assert!(!changed);
    }
}
