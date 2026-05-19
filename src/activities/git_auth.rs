use tokio::process::Command;

pub fn git_command_for_url(url: &str, username: &str, password: &str) -> Command {
    if url.starts_with("http://") || url.starts_with("https://") {
        authenticated_git_command(username, password)
    } else {
        Command::new("git")
    }
}

pub fn authenticated_git_command(username: &str, password: &str) -> Command {
    let mut command = Command::new("git");
    command
        .env("GIT_USERNAME", username)
        .env("GIT_PASSWORD", password)
        .env("GIT_TERMINAL_PROMPT", "0")
        .args([
            "-c",
            "credential.helper=!f() { echo username=$GIT_USERNAME; echo password=$GIT_PASSWORD; }; f",
        ]);
    command
}
