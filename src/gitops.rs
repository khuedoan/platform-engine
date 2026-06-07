mod bundle;
mod create;
mod inventory;
mod manifest;
mod update;

pub(crate) use bundle::{AppsBundle, write_apps_bundle};
pub(crate) use create::{write_add_app_manifests, write_create_app_manifests};
#[cfg(test)]
use inventory::source_repo_from_image;
pub use inventory::{scan_app_inventory, scan_app_source_targets};
pub(crate) use update::update_app_version_inner;

use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AppImageUpdate {
    pub repository: String,
    pub tag: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct UpdateAppVersionInput {
    pub apps_dir: String,
    pub environment: String,
    pub new_images: Vec<AppImageUpdate>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{
        CreateAppRequest, CreateDeployment, CreateHttpRoute, CreateService, CreateVolume, KeyValue,
    };
    use std::fs;
    use std::path::{Path, PathBuf};

    fn write_app_fixture(root: &Path, image: &str) {
        let app_dir = root.join("khuedoan").join("blog").join("production");
        fs::create_dir_all(&app_dir).unwrap();
        fs::write(
            app_dir.join("namespace.yaml"),
            r#"apiVersion: v1
kind: Namespace
metadata:
  name: khuedoan-blog-production
  labels:
    istio.io/dataplane-mode: ambient
"#,
        )
        .unwrap();
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

        assert_eq!(bundle.count, 3);
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
                "wrong namespace filename",
                "namespace-khuedoan-blog-production.yaml",
                r#"apiVersion: v1
kind: Namespace
metadata:
  name: khuedoan-blog-production
"#,
                "expected filename namespace.yaml",
            ),
            (
                "wrong namespace name",
                "namespace.yaml",
                r#"apiVersion: v1
kind: Namespace
metadata:
  name: other
"#,
                "Namespace name must be khuedoan-blog-production",
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
            (
                "missing namespace",
                "service-blog.yaml",
                r#"apiVersion: v1
kind: Service
metadata:
  name: blog
"#,
                "missing namespace.yaml",
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

        assert_eq!(count, 7);
        let namespace = fs::read_to_string(app_dir.join("namespace.yaml")).unwrap();
        assert!(namespace.contains("name: test-example-staging"));
        assert!(namespace.contains("istio.io/dataplane-mode: ambient"));

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

    #[test]
    fn test_write_create_empty_app_environment() {
        let output = PathBuf::from("/tmp/test-cloudlab-create-empty-app");
        let _ = fs::remove_dir_all(&output);
        fs::create_dir_all(&output).unwrap();

        let request = CreateAppRequest {
            tenant: "test".to_string(),
            project: "empty".to_string(),
            environment: "production".to_string(),
            force: false,
            deployment: None,
            service: None,
            http_route: None,
            config: Vec::new(),
            secrets: Vec::new(),
            volumes: Vec::new(),
            postgres: None,
        };

        let app_dir = output.join("test/empty/production");
        fs::create_dir_all(&app_dir).unwrap();
        let count =
            write_create_app_manifests(&app_dir, &request, "registry.registry.svc.cluster.local")
                .unwrap();

        assert_eq!(count, 1);
        assert!(app_dir.join("namespace.yaml").exists());

        let inventory = scan_app_inventory(&output, "registry.registry.svc.cluster.local").unwrap();
        assert_eq!(inventory.len(), 1);
        assert_eq!(inventory[0].tenant, "test");
        assert_eq!(inventory[0].project, "empty");
        assert_eq!(inventory[0].environment, "production");
        assert_eq!(
            inventory[0].resources,
            vec!["Namespace/test-empty-production"]
        );
    }
}
