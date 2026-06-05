use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    process::Command as ProcessCommand,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Parser, Subcommand};
use openidconnect::{
    AdditionalProviderMetadata, AuthType, ClientId, DeviceAuthorizationUrl, IssuerUrl, Nonce,
    OAuth2TokenResponse, ProviderMetadata, Scope, TokenResponse as OidcTokenResponse,
    core::{
        CoreAuthDisplay, CoreClaimName, CoreClaimType, CoreClient, CoreClientAuthMethod,
        CoreDeviceAuthorizationResponse, CoreGrantType, CoreJsonWebKey,
        CoreJweContentEncryptionAlgorithm, CoreJweKeyManagementAlgorithm, CoreResponseMode,
        CoreResponseType, CoreSubjectIdentifierType,
    },
    reqwest as oidc_reqwest,
};
use platform_engine::api::{
    AuthConfig, CreateAppRequest, CreateDeployment, CreateHttpRoute, CreatePostgres, CreateService,
    CreateVolume, DeployRequest, KeyValue, ProjectSummary, UserInfo, WorkflowStarted,
    WorkflowStatus,
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
    List,
    Create(CreateArgs),
    Deploy(DeployArgs),
    Status(StatusArgs),
    Open(OpenArgs),
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
    workflow_id: String,
    #[arg(long)]
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

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let http = Client::new();

    match cli.command {
        Commands::Login => login(&http, cli.server).await,
        Commands::Logout => logout(cli.server),
        Commands::Whoami => {
            let (server, credentials) = server_credentials(cli.server)?;
            let user: UserInfo = api(
                &http,
                Method::GET,
                &server,
                "/api/v1/me",
                &credentials.id_token,
                Option::<&()>::None,
            )
            .await?;
            println!(
                "{}",
                user.username.or(user.email).unwrap_or_else(|| user.subject)
            );
            Ok(())
        }
        Commands::List => {
            let (server, credentials) = server_credentials(cli.server)?;
            let projects: Vec<ProjectSummary> = api(
                &http,
                Method::GET,
                &server,
                "/api/v1/projects",
                &credentials.id_token,
                Option::<&()>::None,
            )
            .await?;
            for project in projects {
                println!(
                    "{}\t{}\t{}\t{}",
                    project.tenant,
                    project.project,
                    project.environment,
                    project.hostnames.join(",")
                );
            }
            Ok(())
        }
        Commands::Create(args) => {
            let request = create_request(args)?;
            let (server, credentials) = server_credentials(cli.server)?;
            let watch = request.1;
            let started: WorkflowStarted = api(
                &http,
                Method::POST,
                &server,
                "/api/v1/apps",
                &credentials.id_token,
                Some(&request.0),
            )
            .await?;
            println!("{}", started.workflow_id);
            if watch {
                watch_workflow(&http, &server, &credentials.id_token, &started.workflow_id).await?;
            }
            Ok(())
        }
        Commands::Deploy(args) => {
            let (server, credentials) = server_credentials(cli.server)?;
            let watch = args.watch;
            let request = deploy_request(args)?;
            let started: WorkflowStarted = api(
                &http,
                Method::POST,
                &server,
                "/api/v1/deployments",
                &credentials.id_token,
                Some(&request),
            )
            .await?;
            println!("{}", started.workflow_id);
            if watch {
                watch_workflow(&http, &server, &credentials.id_token, &started.workflow_id).await?;
            }
            Ok(())
        }
        Commands::Status(args) => {
            let (server, credentials) = server_credentials(cli.server)?;
            if args.watch {
                watch_workflow(&http, &server, &credentials.id_token, &args.workflow_id).await
            } else {
                let status =
                    workflow_status(&http, &server, &credentials.id_token, &args.workflow_id)
                        .await?;
                print_workflow_status(&status);
                Ok(())
            }
        }
        Commands::Open(args) => {
            let (server, credentials) = server_credentials(cli.server)?;
            let status =
                workflow_status(&http, &server, &credentials.id_token, &args.workflow_id).await?;
            let url = status
                .url
                .ok_or_else(|| anyhow!("server did not return a Temporal workflow URL"))?;
            println!("{url}");
            open_url(&url);
            Ok(())
        }
    }
}

async fn login(http: &Client, server: Option<String>) -> Result<()> {
    let mut credentials = load_credentials()?;
    let server = normalize_server(
        server
            .or(credentials.default_server.clone())
            .context("set --server or NETAMOS_URL for the first login")?,
    );
    let auth: AuthConfig = public_api(http, &server, "/api/v1/auth/config").await?;
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
    id_token
        .claims(&client.id_token_verifier(), no_nonce)
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

fn oidc_http_client() -> Result<oidc_reqwest::Client> {
    Ok(oidc_reqwest::ClientBuilder::new()
        .redirect(oidc_reqwest::redirect::Policy::none())
        .build()?)
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

async fn api<T, B>(
    http: &Client,
    method: Method,
    server: &str,
    path: &str,
    token: &str,
    body: Option<B>,
) -> Result<T>
where
    T: DeserializeOwned,
    B: Serialize,
{
    let mut request = http
        .request(method, format!("{}{}", server, path))
        .bearer_auth(token);
    if let Some(body) = body {
        request = request.json(&body);
    }
    decode_api_response(request.send().await?).await
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

fn create_request(args: CreateArgs) -> Result<(CreateAppRequest, bool)> {
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

    Ok((request, args.watch))
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

async fn watch_workflow(http: &Client, server: &str, token: &str, workflow_id: &str) -> Result<()> {
    loop {
        let status = workflow_status(http, server, token, workflow_id).await?;
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

async fn workflow_status(
    http: &Client,
    server: &str,
    token: &str,
    workflow_id: &str,
) -> Result<WorkflowStatus> {
    api(
        http,
        Method::GET,
        server,
        &format!("/api/v1/workflows/{workflow_id}"),
        token,
        Option::<&()>::None,
    )
    .await
}

fn print_workflow_status(status: &WorkflowStatus) {
    if let Some(url) = &status.url {
        println!("{}\t{}\t{}", status.workflow_id, status.status, url);
    } else {
        println!("{}\t{}", status.workflow_id, status.status);
    }
}

fn server_credentials(server: Option<String>) -> Result<(String, ServerCredentials)> {
    let credentials = load_credentials()?;
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
    Ok((server, token))
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
