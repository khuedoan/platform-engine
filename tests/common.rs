use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use serde_json::json;

const FORGEJO_BASE_URL: &str = "http://localhost:3000";
const FORGEJO_ADMIN_USER: &str = "forgejo_admin";
const FORGEJO_ADMIN_PASS: &str = "testing123";

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
        + "&domain=localhost"
        + "&http_port=3000"
        + "&app_url=http%3A%2F%2Flocalhost%3A3000%2F"
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
    let forgejo_base_url = "http://localhost:3000";

    let client = Client::new();

    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

    let check_url = format!("{}/api/v1/repos/{}/{}", forgejo_base_url, repo_owner, repo_name);
    let check_resp = client
        .get(&check_url)
        .basic_auth(FORGEJO_ADMIN_USER, Some(FORGEJO_ADMIN_PASS))
        .headers(headers.clone())
        .send()
        .expect("Failed to check if repo exists");

    if check_resp.status().is_success() {
        println!(
            "Repository '{}/{}' already exists. Skipping migration.",
            repo_owner, repo_name
        );
        return;
    } else if check_resp.status().as_u16() != 404 {
        panic!(
            "Failed to check repository existence: HTTP {} - {}",
            check_resp.status(),
            check_resp.text().unwrap_or_else(|_| "No response body".to_string())
        );
    }

    let api_url = format!("{}/api/v1/repos/migrate", forgejo_base_url);

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
        println!(
            "Repository '{}' successfully migrated to owner '{}'",
            repo_name, repo_owner
        );
    } else {
        let status = response.status();
        let text = response.text().unwrap_or_else(|_| "No response body".to_string());
        panic!("Failed to migrate repository: HTTP {} - {}", status, text);
    }
}

pub fn setup() {
    init_forgejo();
    std::thread::sleep(core::time::Duration::from_secs(5));

    migrate_repo("https://github.com/khuedoan/cloudlab", FORGEJO_ADMIN_USER, "gitops");
    migrate_repo("https://github.com/khuedoan/cloudlab", FORGEJO_ADMIN_USER, "blog");
}
