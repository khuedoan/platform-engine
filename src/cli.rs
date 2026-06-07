use std::{
    collections::BTreeMap,
    fs,
    io::{self, IsTerminal},
    path::PathBuf,
    process::Command as ProcessCommand,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::api::{
    AuthConfig, CreateAppRequest, CreateDeployment, CreateHttpRoute, CreatePostgres, CreateService,
    CreateVolume, DeleteAppRequest, DeployRequest, KeyValue, ProjectSummary, UserInfo,
    WorkflowStarted, WorkflowStatus, deploy_workflow_id,
};
use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Parser, Subcommand};
use comfy_table::{Attribute, Cell, ContentArrangement, Table, presets::NOTHING};
use inquire::Text;
use openidconnect::{
    AdditionalProviderMetadata, AuthType, ClientId, DeviceAuthorizationUrl, IssuerUrl, Nonce,
    OAuth2TokenResponse, ProviderMetadata, RefreshToken, Scope, TokenResponse as OidcTokenResponse,
    core::{
        CoreAuthDisplay, CoreClaimName, CoreClaimType, CoreClient, CoreClientAuthMethod,
        CoreDeviceAuthorizationResponse, CoreGrantType, CoreJsonWebKey,
        CoreJweContentEncryptionAlgorithm, CoreJweKeyManagementAlgorithm, CoreResponseMode,
        CoreResponseType, CoreSubjectIdentifierType,
    },
    reqwest as oidc_reqwest,
};
use reqwest::{Client, Method};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::time::sleep;

#[derive(Parser)]
#[command(name = "netamos")]
struct Cli {
    #[arg(long, env = "NETAMOS_URL", global = true)]
    server: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Login,
    Logout,
    Whoami,
    List(ListArgs),
    Create(CreateArgs),
    Delete(DeleteArgs),
    Deploy(DeployArgs),
    Status(StatusArgs),
    Open(OpenArgs),
}

#[derive(Args)]
struct ListArgs {
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct CreateArgs {
    #[arg(long)]
    tenant: String,
    #[arg(long)]
    project: String,
    #[arg(long)]
    environment: String,
    #[arg(long)]
    force: bool,
    #[arg(long)]
    image: Option<String>,
    #[arg(long)]
    source_repo: Option<String>,
    #[arg(long, default_value_t = 1)]
    replicas: u32,
    #[arg(long)]
    port: Option<u16>,
    #[arg(long)]
    service: bool,
    #[arg(long)]
    hostname: Option<String>,
    #[arg(long)]
    route_port: Option<u16>,
    #[arg(long = "config")]
    config: Vec<String>,
    #[arg(long = "secret")]
    secrets: Vec<String>,
    #[arg(long = "volume")]
    volumes: Vec<String>,
    #[arg(long)]
    postgres: bool,
    #[arg(long, default_value = "1Gi")]
    postgres_size: String,
    #[arg(long = "watch", hide = true)]
    watch: bool,
}

#[derive(Args)]
struct DeleteArgs {
    #[arg(long)]
    tenant: String,
    #[arg(long)]
    project: String,
    #[arg(long)]
    environment: String,
    #[arg(long)]
    watch: bool,
}

#[derive(Args)]
struct DeployArgs {
    #[arg(long)]
    repo: Option<String>,
    #[arg(long)]
    revision: Option<String>,
    #[arg(long)]
    environment: String,
    #[arg(long)]
    watch: bool,
}

#[derive(Args)]
struct StatusArgs {
    #[arg(
        long,
        value_name = "REV",
        help = "Git revision to check; defaults to HEAD"
    )]
    commit: Option<String>,
    #[arg(long, help = "Poll until the deployment workflow finishes")]
    watch: bool,
}

#[derive(Args)]
struct OpenArgs {
    workflow_id: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct CredentialsFile {
    default_server: Option<String>,
    servers: BTreeMap<String, ServerCredentials>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ServerCredentials {
    id_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_at: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct DeviceProviderFields {
    device_authorization_endpoint: DeviceAuthorizationUrl,
}

impl AdditionalProviderMetadata for DeviceProviderFields {}

type DeviceProviderMetadata = ProviderMetadata<
    DeviceProviderFields,
    CoreAuthDisplay,
    CoreClientAuthMethod,
    CoreClaimName,
    CoreClaimType,
    CoreGrantType,
    CoreJweContentEncryptionAlgorithm,
    CoreJweKeyManagementAlgorithm,
    CoreJsonWebKey,
    CoreResponseMode,
    CoreResponseType,
    CoreSubjectIdentifierType,
>;

#[derive(Deserialize)]
struct ApiErrorBody {
    error: String,
}

pub async fn run() -> Result<()> {
    let cli = Cli::parse();
    let http = Client::new();

    match cli.command {
        Commands::Login => login(&http, cli.server).await,
        Commands::Logout => logout(cli.server),
        Commands::Whoami => {
            let api = ApiSession::load(&http, cli.server).await?;
            let user: UserInfo = api.get("/api/v1/me").await?;
            println!("{}", user.username.or(user.email).unwrap_or(user.subject));
            Ok(())
        }
        Commands::List(args) => {
            let api = ApiSession::load(&http, cli.server).await?;
            let projects: Vec<ProjectSummary> = api.get("/api/v1/projects").await?;
            if args.json {
                println!("{}", serde_json::to_string_pretty(&projects)?);
            } else {
                print_projects(&projects);
            }
            Ok(())
        }
        Commands::Create(args) => {
            let request = create_request(args)?;
            let api = ApiSession::load(&http, cli.server).await?;
            let started: WorkflowStarted = api.post("/api/v1/apps", &request).await?;
            println!("{}", started.workflow_id);
            api.watch_workflow(&started.workflow_id).await?;
            Ok(())
        }
        Commands::Delete(args) => {
            let (request, watch) = delete_request(args)?;
            let api = ApiSession::load(&http, cli.server).await?;
            let path = format!(
                "/api/v1/apps/{}/{}/{}",
                request.tenant, request.project, request.environment
            );
            let started: WorkflowStarted = api.delete(&path).await?;
            println!("{}", started.workflow_id);
            if watch {
                api.watch_workflow(&started.workflow_id).await?;
            }
            Ok(())
        }
        Commands::Deploy(args) => {
            let watch = args.watch;
            let request = deploy_request(args)?;
            let api = ApiSession::load(&http, cli.server).await?;
            let started: WorkflowStarted = api.post("/api/v1/deployments", &request).await?;
            println!("{}", started.workflow_id);
            if watch {
                api.watch_workflow(&started.workflow_id).await?;
            }
            Ok(())
        }
        Commands::Status(args) => {
            let commit = git_commit(args.commit.as_deref())?;
            let repo = repo_from_git_remote()?;
            let repo_name = repo_name_from_slug(&repo)?;
            let workflow_id = deploy_workflow_id(repo_name, &commit);
            let api = ApiSession::load(&http, cli.server).await?;
            if args.watch {
                api.watch_commit_workflow(&commit, &workflow_id).await
            } else {
                let status = api.workflow_status(&workflow_id).await?;
                print_commit_status(&commit, &status);
                Ok(())
            }
        }
        Commands::Open(args) => {
            let api = ApiSession::load(&http, cli.server).await?;
            let status = api.workflow_status(&args.workflow_id).await?;
            let url = status
                .url
                .ok_or_else(|| anyhow!("server did not return a Temporal workflow URL"))?;
            println!("{url}");
            open_url(&url);
            Ok(())
        }
    }
}

struct ApiSession<'a> {
    http: &'a Client,
    server: String,
    token: String,
}

impl<'a> ApiSession<'a> {
    async fn load(http: &'a Client, server: Option<String>) -> Result<Self> {
        let (server, credentials) = server_credentials(http, server).await?;
        Ok(Self {
            http,
            server,
            token: credentials.id_token,
        })
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.request(Method::GET, path, Option::<&()>::None).await
    }

    async fn post<T, B>(&self, path: &str, body: &B) -> Result<T>
    where
        T: DeserializeOwned,
        B: Serialize,
    {
        self.request(Method::POST, path, Some(body)).await
    }

    async fn delete<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.request(Method::DELETE, path, Option::<&()>::None)
            .await
    }

    async fn request<T, B>(&self, method: Method, path: &str, body: Option<B>) -> Result<T>
    where
        T: DeserializeOwned,
        B: Serialize,
    {
        let mut request = self
            .http
            .request(method, format!("{}{}", self.server, path))
            .bearer_auth(&self.token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        decode_api_response(request.send().await?).await
    }

    async fn workflow_status(&self, workflow_id: &str) -> Result<WorkflowStatus> {
        self.get(&format!("/api/v1/workflows/{workflow_id}")).await
    }

    async fn watch_workflow(&self, workflow_id: &str) -> Result<()> {
        loop {
            let status = self.workflow_status(workflow_id).await?;
            print_workflow_status(&status);
            if status.is_terminal() {
                if status.status == "completed" {
                    return Ok(());
                }
                bail!("workflow ended with status {}", status.status);
            }
            sleep(Duration::from_secs(5)).await;
        }
    }

    async fn watch_commit_workflow(&self, commit: &str, workflow_id: &str) -> Result<()> {
        loop {
            let status = self.workflow_status(workflow_id).await?;
            print_commit_status(commit, &status);
            if status.is_terminal() {
                if status.status == "completed" {
                    return Ok(());
                }
                bail!("workflow ended with status {}", status.status);
            }
            sleep(Duration::from_secs(5)).await;
        }
    }
}

async fn login(http: &Client, server: Option<String>) -> Result<()> {
    let mut credentials = load_credentials()?;
    let server = login_server(server, credentials.default_server.clone())?;
    let auth: AuthConfig = public_api(http, &server, "/api/v1/auth/config").await?;
    let trusted_audience = auth
        .audience
        .clone()
        .or_else(|| trusted_audience_from_scopes(&auth.scopes));
    let oidc_http = oidc_http_client()?;
    let provider =
        DeviceProviderMetadata::discover_async(IssuerUrl::new(auth.issuer)?, &oidc_http).await?;
    let device_authorization_endpoint = provider
        .additional_metadata()
        .device_authorization_endpoint
        .clone();
    let client = CoreClient::from_provider_metadata(provider, ClientId::new(auth.client_id), None)
        .set_device_authorization_url(device_authorization_endpoint)
        .set_auth_type(AuthType::RequestBody);

    let device: CoreDeviceAuthorizationResponse = client
        .exchange_device_code()
        .add_scopes(auth.scopes.into_iter().map(Scope::new))
        .request_async(&oidc_http)
        .await?;

    if let Some(url) = device.verification_uri_complete() {
        println!("{}", url.secret());
    } else {
        println!("{}", device.verification_uri());
        println!("{}", device.user_code().secret());
    }

    let token = client
        .exchange_device_access_token(&device)?
        .request_async(&oidc_http, sleep, None)
        .await?;
    let id_token = token
        .id_token()
        .context("issuer did not return an ID token")?
        .to_owned();
    let mut verifier = client.id_token_verifier();
    if let Some(audience) = trusted_audience {
        verifier = verifier.set_other_audience_verifier_fn(move |aud| **aud == audience);
    }
    id_token
        .claims(&verifier, no_nonce)
        .context("validate ID token")?;

    credentials.default_server = Some(server.clone());
    credentials.servers.insert(
        server,
        ServerCredentials {
            id_token: id_token.to_string(),
            refresh_token: token
                .refresh_token()
                .map(|refresh_token| refresh_token.secret().to_string()),
            expires_at: token
                .expires_in()
                .map(|expires| now_seconds() + expires.as_secs()),
        },
    );
    save_credentials(&credentials)?;
    Ok(())
}

fn login_server(server: Option<String>, default_server: Option<String>) -> Result<String> {
    if let Some(server) = server.or(default_server) {
        return Ok(normalize_server(server));
    }

    if !io::stdin().is_terminal() {
        bail!("set --server or NETAMOS_URL for non-interactive login");
    }

    let server = Text::new("Server URL").prompt()?;
    let server = server.trim();
    if server.is_empty() {
        bail!("server URL is required");
    }

    Ok(normalize_server(server.to_string()))
}

fn oidc_http_client() -> Result<oidc_reqwest::Client> {
    Ok(oidc_reqwest::ClientBuilder::new()
        .redirect(oidc_reqwest::redirect::Policy::none())
        .build()?)
}

fn trusted_audience_from_scopes(scopes: &[String]) -> Option<String> {
    scopes
        .iter()
        .find_map(|scope| scope.strip_prefix("audience:server:client_id:"))
        .map(ToString::to_string)
}

fn no_nonce(nonce: Option<&Nonce>) -> std::result::Result<(), String> {
    match nonce {
        Some(_) => Err("unexpected nonce claim".to_string()),
        None => Ok(()),
    }
}

fn logout(server: Option<String>) -> Result<()> {
    let mut credentials = load_credentials()?;
    let server = normalize_server(
        server
            .or(credentials.default_server.clone())
            .context("no server is configured")?,
    );
    credentials.servers.remove(&server);
    if credentials.default_server.as_deref() == Some(&server) {
        credentials.default_server = credentials.servers.keys().next().cloned();
    }
    save_credentials(&credentials)?;
    Ok(())
}

async fn public_api<T: DeserializeOwned>(http: &Client, server: &str, path: &str) -> Result<T> {
    let response = http.get(format!("{}{}", server, path)).send().await?;
    decode_api_response(response).await
}

async fn decode_api_response<T: DeserializeOwned>(response: reqwest::Response) -> Result<T> {
    let status = response.status();
    if status.is_success() {
        return Ok(response.json().await?);
    }

    let body = response
        .json::<ApiErrorBody>()
        .await
        .map(|body| body.error)
        .unwrap_or_else(|_| status.to_string());
    Err(anyhow!("request failed: {body}"))
}

fn create_request(args: CreateArgs) -> Result<CreateAppRequest> {
    let _ = args.watch;
    let config = parse_key_values(args.config)?;
    let secrets = parse_key_values(args.secrets)?;
    let volumes = parse_volumes(args.volumes)?;
    let deployment = if args.image.is_some() || args.source_repo.is_some() || args.port.is_some() {
        Some(CreateDeployment {
            image: args.image,
            source_repo: args.source_repo,
            replicas: args.replicas,
            port: args.port,
        })
    } else {
        None
    };
    let service = if args.service || args.hostname.is_some() {
        Some(CreateService {
            port: args
                .port
                .or(args.route_port)
                .context("--service needs --port")?,
        })
    } else {
        None
    };
    let http_route = if let Some(hostname) = args.hostname {
        Some(CreateHttpRoute {
            hostname,
            port: args
                .route_port
                .or(args.port)
                .context("--hostname needs --port or --route-port")?,
        })
    } else {
        None
    };
    let postgres = args.postgres.then_some(CreatePostgres {
        size: args.postgres_size,
    });

    let request = CreateAppRequest {
        tenant: args.tenant,
        project: args.project,
        environment: args.environment,
        force: args.force,
        deployment,
        service,
        http_route,
        config,
        secrets,
        volumes,
        postgres,
    };
    request.validate().map_err(anyhow::Error::msg)?;

    Ok(request)
}

fn deploy_request(args: DeployArgs) -> Result<DeployRequest> {
    Ok(DeployRequest {
        repo: match args.repo {
            Some(repo) => repo,
            None => repo_from_git_remote()?,
        },
        revision: match args.revision {
            Some(revision) => revision,
            None => git_output(["rev-parse", "HEAD"])?,
        },
        environment: args.environment,
    })
}

fn delete_request(args: DeleteArgs) -> Result<(DeleteAppRequest, bool)> {
    let request = DeleteAppRequest {
        tenant: args.tenant,
        project: args.project,
        environment: args.environment,
    };
    request.validate().map_err(anyhow::Error::msg)?;
    Ok((request, args.watch))
}

fn git_commit(commit: Option<&str>) -> Result<String> {
    git_output(["rev-parse", commit.unwrap_or("HEAD")])
}

fn repo_name_from_slug(repo: &str) -> Result<&str> {
    repo.rsplit('/')
        .next()
        .filter(|repo| !repo.is_empty())
        .ok_or_else(|| anyhow!("repo must be in owner/name form"))
}

fn print_workflow_status(status: &WorkflowStatus) {
    if let Some(url) = &status.url {
        println!("{}\t{}\t{}", status.workflow_id, status.status, url);
    } else {
        println!("{}\t{}", status.workflow_id, status.status);
    }
}

fn print_commit_status(commit: &str, status: &WorkflowStatus) {
    let commit = commit.chars().take(12).collect::<String>();
    if let Some(url) = &status.url {
        println!("{commit}\t{}\t{url}", status.status);
    } else {
        println!("{commit}\t{}", status.status);
    }
}

fn print_projects(projects: &[ProjectSummary]) {
    let header = ["TENANT", "PROJECT", "ENV", "DOMAINS"]
        .map(|title| Cell::new(title).add_attribute(Attribute::Bold));
    let mut table = Table::new();
    table
        .load_preset(NOTHING)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(header);

    for project in projects {
        table.add_row([
            project.tenant.as_str(),
            project.project.as_str(),
            project.environment.as_str(),
            &project.hostnames.join(", "),
        ]);
    }

    println!("{table}");
}

async fn server_credentials(
    http: &Client,
    server: Option<String>,
) -> Result<(String, ServerCredentials)> {
    let mut credentials = load_credentials()?;
    let server = normalize_server(
        server
            .or(credentials.default_server.clone())
            .context("run netamos login first or set --server")?,
    );
    let token = credentials
        .servers
        .get(&server)
        .cloned()
        .ok_or_else(|| anyhow!("run netamos login for {server}"))?;
    if !token_needs_refresh(&token) {
        return Ok((server, token));
    }

    let token = refresh_credentials(http, &server, &token).await?;
    credentials.servers.insert(server.clone(), token.clone());
    save_credentials(&credentials)?;
    Ok((server, token))
}

fn token_needs_refresh(credentials: &ServerCredentials) -> bool {
    credentials
        .expires_at
        .is_some_and(|expires_at| expires_at <= now_seconds().saturating_add(60))
}

async fn refresh_credentials(
    http: &Client,
    server: &str,
    credentials: &ServerCredentials,
) -> Result<ServerCredentials> {
    let refresh_token = credentials
        .refresh_token
        .clone()
        .ok_or_else(|| anyhow!("session expired; run netamos login for {server}"))?;
    let auth: AuthConfig = public_api(http, server, "/api/v1/auth/config").await?;
    let trusted_audience = auth
        .audience
        .clone()
        .or_else(|| trusted_audience_from_scopes(&auth.scopes));
    let oidc_http = oidc_http_client()?;
    let provider =
        DeviceProviderMetadata::discover_async(IssuerUrl::new(auth.issuer)?, &oidc_http).await?;
    let device_authorization_endpoint = provider
        .additional_metadata()
        .device_authorization_endpoint
        .clone();
    let client = CoreClient::from_provider_metadata(provider, ClientId::new(auth.client_id), None)
        .set_device_authorization_url(device_authorization_endpoint)
        .set_auth_type(AuthType::RequestBody);

    let token = client
        .exchange_refresh_token(&RefreshToken::new(refresh_token.clone()))?
        .request_async(&oidc_http)
        .await?;
    let id_token = token
        .id_token()
        .context("issuer did not return an ID token")?
        .to_owned();
    let mut verifier = client.id_token_verifier();
    if let Some(audience) = trusted_audience {
        verifier = verifier.set_other_audience_verifier_fn(move |aud| **aud == audience);
    }
    id_token
        .claims(&verifier, no_nonce)
        .context("validate refreshed ID token")?;

    Ok(ServerCredentials {
        id_token: id_token.to_string(),
        refresh_token: token
            .refresh_token()
            .map(|refresh_token| refresh_token.secret().to_string())
            .or(Some(refresh_token)),
        expires_at: token
            .expires_in()
            .map(|expires| now_seconds() + expires.as_secs()),
    })
}

fn load_credentials() -> Result<CredentialsFile> {
    let path = credentials_path()?;
    if !path.exists() {
        return Ok(CredentialsFile::default());
    }
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

fn save_credentials(credentials: &CredentialsFile) -> Result<()> {
    let path = credentials_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec_pretty(credentials)?)?;
    Ok(())
}

fn credentials_path() -> Result<PathBuf> {
    Ok(
        PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?)
            .join(".config/netamos/credentials.json"),
    )
}

fn parse_key_values(values: Vec<String>) -> Result<Vec<KeyValue>> {
    values
        .into_iter()
        .map(|value| {
            let (key, value) = value
                .split_once('=')
                .ok_or_else(|| anyhow!("{value}: expected KEY=VALUE"))?;
            Ok(KeyValue {
                key: key.to_string(),
                value: value.to_string(),
            })
        })
        .collect()
}

fn parse_volumes(values: Vec<String>) -> Result<Vec<CreateVolume>> {
    values
        .into_iter()
        .map(|value| {
            let parts = value.splitn(3, ':').collect::<Vec<_>>();
            if parts.len() != 3 {
                bail!("{value}: expected name:size:/mount/path");
            }
            Ok(CreateVolume {
                name: parts[0].to_string(),
                size: parts[1].to_string(),
                mount_path: parts[2].to_string(),
            })
        })
        .collect()
}

fn repo_from_git_remote() -> Result<String> {
    repo_slug_from_git_url(&git_output(["config", "--get", "remote.origin.url"])?)
        .ok_or_else(|| anyhow!("could not infer owner/repo from remote.origin.url"))
}

fn git_output<const N: usize>(args: [&str; N]) -> Result<String> {
    let output = ProcessCommand::new("git").args(args).output()?;
    if !output.status.success() {
        bail!(
            "git command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn repo_slug_from_git_url(url: &str) -> Option<String> {
    let url = url.trim_end_matches(".git");
    let path = if let Some((_, rest)) = url.split_once("://") {
        rest.split_once('/').map(|(_, path)| path)?
    } else if let Some((_, path)) = url.split_once(':') {
        path
    } else {
        url
    };

    let mut parts = path.trim_matches('/').rsplitn(2, '/');
    let repo = parts.next()?.trim();
    let owner = parts.next()?.trim();
    if owner.is_empty() || repo.is_empty() {
        return None;
    }

    Some(format!("{owner}/{repo}"))
}

fn normalize_server(server: String) -> String {
    server.trim_end_matches('/').to_string()
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn open_url(url: &str) {
    if cfg!(target_os = "macos") {
        let _ = ProcessCommand::new("open").arg(url).status();
    } else {
        let _ = ProcessCommand::new("xdg-open").arg(url).status();
    }
}
