use reqwest::blocking::Client;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::json;

pub const FORGEJO_BASE_URL: &str = "http://localhost:3000";
pub const FORGEJO_ADMIN_USER: &str = "khuedoan";
pub const FORGEJO_ADMIN_PASS: &str = "testing123";

fn init_forgejo() {
    let mut headers = HeaderMap::new();
    let client = Client::new();
    headers.insert(
        "Content-Type",
        "application/x-www-form-urlencoded".parse().unwrap(),
    );

    // We use an ephermeral Forgejo instance for integration test,
    // so it's fine to use plaintext passwords here.
    let body = "db_type=sqlite3".to_string()
        + "&db_host=localhost%3A3306"
        + "&db_user=root"
        + "&db_passwd="
        + "&db_name=gitea"
        + "&ssl_mode=disable"
        + "&db_schema="
        + "&charset=utf8"
        + "&db_path=%2Fdata%2Fgitea%2Fgitea.db"
        + "&app_name=Gitea%3A+Git+with+a+cup+of+tea"
        + "&repo_root_path=%2Fdata%2Fgit%2Frepositories"
        + "&run_user=git"
        + "&domain=git"
        + "&http_port=3000"
        + "&app_url=http%3A%2F%2Fgit%3A3000%2F"
        + "&log_root_path=%2Fdata%2Fgitea%2Flog"
        + "&no_reply_address=noreply.localhost"
        + "&password_algorithm=pbkdf2"
        + &format!("&admin_name={FORGEJO_ADMIN_USER}")
        + &format!("&admin_passwd={FORGEJO_ADMIN_PASS}")
        + &format!("&admin_confirm_passwd={FORGEJO_ADMIN_PASS}")
        + "&admin_email=admin%40example.com";

    client
        .post(FORGEJO_BASE_URL)
        .headers(headers.clone())
        .body(body)
        .send()
        .unwrap();
}

fn migrate_repo(clone_addr: &str, repo_owner: &str, repo_name: &str) {
    let forgejo_base_url = FORGEJO_BASE_URL;

    let client = Client::new();

    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

    let check_url = format!("{forgejo_base_url}/api/v1/repos/{repo_owner}/{repo_name}");
    let check_resp = client
        .get(&check_url)
        .basic_auth(FORGEJO_ADMIN_USER, Some(FORGEJO_ADMIN_PASS))
        .headers(headers.clone())
        .send()
        .expect("Failed to check if repo exists");

    if check_resp.status().is_success() {
        println!("Repository '{repo_owner}/{repo_name}' already exists. Skipping migration.");
        return;
    } else if check_resp.status().as_u16() != 404 {
        panic!(
            "Failed to check repository existence: HTTP {} - {}",
            check_resp.status(),
            check_resp
                .text()
                .unwrap_or_else(|_| "No response body".to_string())
        );
    }

    let api_url = format!("{forgejo_base_url}/api/v1/repos/migrate");

    let body = json!({
        "clone_addr": clone_addr,
        "repo_owner": repo_owner,
        "repo_name": repo_name,
    });

    let response = client
        .post(&api_url)
        .basic_auth(FORGEJO_ADMIN_USER, Some(FORGEJO_ADMIN_PASS))
        .headers(headers)
        .json(&body)
        .send()
        .expect("Failed to send migration request");

    if response.status().is_success() {
        println!("Repository '{repo_name}' successfully migrated to owner '{repo_owner}'");
    } else {
        let status = response.status();
        let text = response
            .text()
            .unwrap_or_else(|_| "No response body".to_string());
        panic!("Failed to migrate repository: HTTP {status} - {text}");
    }
}

fn create_repo(repo_owner: &str, repo_name: &str) {
    let client = Client::new();

    let check_url = format!("{FORGEJO_BASE_URL}/api/v1/repos/{repo_owner}/{repo_name}");

    let check_resp = client
        .get(&check_url)
        .basic_auth(FORGEJO_ADMIN_USER, Some(FORGEJO_ADMIN_PASS))
        .send()
        .expect("Failed to check if repo exists");

    if check_resp.status().is_success() {
        println!("Repository '{repo_owner}/{repo_name}' already exists. Skipping creation.");
        return;
    } else if check_resp.status().as_u16() != 404 {
        panic!(
            "Failed to check repository existence: HTTP {} - {}",
            check_resp.status(),
            check_resp
                .text()
                .unwrap_or_else(|_| "No response body".to_string())
        );
    }

    // Repository doesn't exist — create it
    let api_url = format!("{FORGEJO_BASE_URL}/api/v1/admin/users/{repo_owner}/repos");

    let body = json!({
        "name": repo_name,
        "description": "Auto-created by integration test",
        "private": false,
    });

    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

    let resp = client
        .post(&api_url)
        .basic_auth(FORGEJO_ADMIN_USER, Some(FORGEJO_ADMIN_PASS))
        .headers(headers)
        .json(&body)
        .send()
        .expect("Failed to send repo creation request");

    if resp.status().is_success() {
        println!("Repository '{repo_owner}/{repo_name}' successfully created.");
    } else {
        let status = resp.status();
        let text = resp
            .text()
            .unwrap_or_else(|_| "No response body".to_string());
        panic!("Failed to create repository: HTTP {status} - {text}");
    }
}

fn setup_webhook(repo_owner: &str, repo_name: &str) {
    let client = Client::new();
    let hooks_url = format!("{FORGEJO_BASE_URL}/api/v1/repos/{repo_owner}/{repo_name}/hooks");

    let target_url = "http://server:8080/webhooks/gitea";

    // Step 1: Fetch existing webhooks
    let resp = client
        .get(&hooks_url)
        .basic_auth(FORGEJO_ADMIN_USER, Some(FORGEJO_ADMIN_PASS))
        .send()
        .expect("Failed to fetch existing webhooks");

    if !resp.status().is_success() {
        panic!(
            "Failed to list webhooks: HTTP {} - {}",
            resp.status(),
            resp.text().unwrap_or_default()
        );
    }

    let hooks: serde_json::Value = resp.json().expect("Invalid JSON from hooks API");
    let already_exists = hooks.as_array().unwrap_or(&vec![]).iter().any(|hook| {
        hook.get("config")
            .and_then(|cfg| cfg.get("url"))
            .and_then(|url| url.as_str())
            .map(|url| url == target_url)
            .unwrap_or(false)
    });

    if already_exists {
        println!("Webhook already exists for '{repo_owner}/{repo_name}'. Skipping creation.");
        return;
    }

    // Step 2: Create the webhook
    let body = json!({
        "type": "gitea",
        "config": {
            "url": target_url,
            "content_type": "json"
        },
        "events": ["push"],
        "active": true
    });

    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

    let create_resp = client
        .post(&hooks_url)
        .basic_auth(FORGEJO_ADMIN_USER, Some(FORGEJO_ADMIN_PASS))
        .headers(headers)
        .json(&body)
        .send()
        .expect("Failed to create webhook");

    if create_resp.status().is_success() {
        println!("Webhook created successfully for '{repo_owner}/{repo_name}'");
    } else {
        let status = create_resp.status();
        let text = create_resp
            .text()
            .unwrap_or_else(|_| "No response body".to_string());
        panic!("Failed to create webhook: HTTP {status} - {text}");
    }
}

pub fn setup() {
    init_forgejo();
    std::thread::sleep(core::time::Duration::from_secs(5));

    migrate_repo(
        "https://github.com/khuedoan/cloudlab",
        FORGEJO_ADMIN_USER,
        "cloudlab",
    );

    create_repo(FORGEJO_ADMIN_USER, "blog");

    setup_webhook(FORGEJO_ADMIN_USER, "blog");
}
