use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    pub issuer: String,
    pub client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audience: Option<String>,
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInfo {
    pub subject: String,
    pub username: Option<String>,
    pub email: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectSummary {
    pub tenant: String,
    pub project: String,
    pub environment: String,
    pub resources: Vec<String>,
    pub hostnames: Vec<String>,
    pub images: Vec<String>,
    pub source_repos: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowStarted {
    pub workflow_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowStatus {
    pub workflow_id: String,
    pub status: String,
    pub url: Option<String>,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
}

impl WorkflowStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status.as_str(),
            "completed" | "failed" | "canceled" | "terminated" | "timed_out"
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateAppRequest {
    pub tenant: String,
    pub project: String,
    pub environment: String,
    pub force: bool,
    pub deployment: Option<CreateDeployment>,
    pub service: Option<CreateService>,
    pub http_route: Option<CreateHttpRoute>,
    pub config: Vec<KeyValue>,
    pub secrets: Vec<KeyValue>,
    pub volumes: Vec<CreateVolume>,
    pub postgres: Option<CreatePostgres>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteAppRequest {
    pub tenant: String,
    pub project: String,
    pub environment: String,
}

impl DeleteAppRequest {
    pub fn app_path(&self) -> String {
        format!("{}/{}/{}", self.tenant, self.project, self.environment)
    }

    pub fn validate(&self) -> Result<(), String> {
        validate_dns_name("tenant", &self.tenant)?;
        validate_dns_name("project", &self.project)?;
        validate_dns_name("environment", &self.environment)?;
        Ok(())
    }
}

impl CreateAppRequest {
    pub fn app_path(&self) -> String {
        format!("{}/{}/{}", self.tenant, self.project, self.environment)
    }

    pub fn validate(&self) -> Result<(), String> {
        validate_dns_name("tenant", &self.tenant)?;
        validate_dns_name("project", &self.project)?;
        validate_dns_name("environment", &self.environment)?;
        if let Some(deployment) = &self.deployment
            && deployment.image.is_none()
            && deployment.source_repo.is_none()
        {
            return Err("deployment needs either an image or a source repo".to_string());
        }
        for item in self.config.iter().chain(self.secrets.iter()) {
            validate_env_key(&item.key)?;
        }
        for volume in &self.volumes {
            validate_dns_name("volume name", &volume.name)?;
            if volume.size.trim().is_empty() {
                return Err(format!("volume {} needs a storage size", volume.name));
            }
            if !volume.mount_path.starts_with('/') {
                return Err(format!(
                    "volume {} mount path must be absolute",
                    volume.name
                ));
            }
        }
        if let Some(postgres) = &self.postgres
            && postgres.size.trim().is_empty()
        {
            return Err("postgres needs a storage size".to_string());
        }

        Ok(())
    }
}

fn validate_dns_name(field: &str, value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("{field} is required"));
    }
    if value.len() > 63 {
        return Err(format!("{field} must be at most 63 characters"));
    }
    if !value
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        return Err(format!(
            "{field} must contain only lowercase letters, digits, and '-'"
        ));
    }
    if !value
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit())
        || !value
            .chars()
            .last()
            .is_some_and(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit())
    {
        return Err(format!("{field} must start and end with a letter or digit"));
    }
    Ok(())
}

fn validate_env_key(value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err("config and secret keys cannot be empty".to_string());
    }
    if !value
        .chars()
        .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '_')
    {
        return Err(format!(
            "{value} must contain only uppercase letters, digits, and '_'"
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateDeployment {
    pub image: Option<String>,
    pub source_repo: Option<String>,
    pub replicas: u32,
    pub port: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateService {
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateHttpRoute {
    pub hostname: String,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyValue {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateVolume {
    pub name: String,
    pub size: String,
    pub mount_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatePostgres {
    pub size: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployRequest {
    pub repo: String,
    pub revision: String,
    pub environment: String,
}

pub fn deploy_workflow_id(repo_name: &str, revision: &str) -> String {
    format!(
        "push-to-deploy-{}-{}",
        sanitize_workflow_part(repo_name),
        revision.chars().take(12).collect::<String>()
    )
}

fn sanitize_workflow_part(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
            out.push(ch);
        } else if ch.is_whitespace() || ch == '/' {
            out.push('-');
        }
    }
    out.trim_matches('-').to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_request() -> CreateAppRequest {
        CreateAppRequest {
            tenant: "test".to_string(),
            project: "example".to_string(),
            environment: "production".to_string(),
            force: false,
            deployment: None,
            service: None,
            http_route: None,
            config: Vec::new(),
            secrets: Vec::new(),
            volumes: Vec::new(),
            postgres: None,
        }
    }

    #[test]
    fn create_app_request_accepts_empty_resource_set() {
        empty_request().validate().unwrap();
    }

    #[test]
    fn create_app_request_accepts_source_deployment() {
        let mut request = empty_request();
        request.deployment = Some(CreateDeployment {
            image: None,
            source_repo: Some("khuedoan/example-service".to_string()),
            replicas: 1,
            port: Some(3000),
        });

        request.validate().unwrap();
    }

    #[test]
    fn delete_app_request_validates_app_path() {
        let request = DeleteAppRequest {
            tenant: "test".to_string(),
            project: "example".to_string(),
            environment: "production".to_string(),
        };

        request.validate().unwrap();
        assert_eq!(request.app_path(), "test/example/production");
    }

    #[test]
    fn deploy_workflow_id_matches_push_convention() {
        assert_eq!(
            deploy_workflow_id("example-service", "6c1c137dc62d1234567890"),
            "push-to-deploy-example-service-6c1c137dc62d"
        );
    }
}
