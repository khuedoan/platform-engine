use super::manifest::{validate_app_manifest, write_yaml_manifest};
use crate::api::{
    CreateAppRequest, CreateDeployment, CreateHttpRoute, CreatePostgres, CreateService,
    CreateVolume, KeyValue,
};
use anyhow::Context;
use serde_json::{Value as JsonValue, json};
use std::path::Path;

pub(crate) fn write_create_app_manifests(
    app_dir: &Path,
    request: &CreateAppRequest,
    registry: &str,
) -> anyhow::Result<usize> {
    write_json_manifest(&app_dir.join("namespace.yaml"), namespace_manifest(request))?;
    let mut count = 1;

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

fn namespace_manifest(request: &CreateAppRequest) -> JsonValue {
    json!({
        "apiVersion": "v1",
        "kind": "Namespace",
        "metadata": {
            "name": request.app_path().replace('/', "-"),
            "labels": { "istio.io/dataplane-mode": "ambient" },
        },
    })
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
                    "labels": { "app.kubernetes.io/name": &request.project },
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
    let manifest = yaml_serde::to_value(value)?;
    validate_app_manifest(path, &manifest)?;
    write_yaml_manifest(path, &manifest)
}
