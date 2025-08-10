use std::process::Command;

mod common;

fn push_to_forgejo() {
    let test_repo_path = "testdata/example-service";

    // HTTP remote with credentials embedded
    let remote_url = format!(
        "http://{}:{}@localhost:3000/{}/example-service.git",
        common::FORGEJO_ADMIN_USER,
        common::FORGEJO_ADMIN_PASS,
        common::FORGEJO_ADMIN_USER
    );

    // Set remote
    let remote_add = Command::new("git")
        .args(["remote", "add", "test", &remote_url])
        .current_dir(test_repo_path)
        .output()
        .expect("failed to add remote");

    if !remote_add.status.success()
        && !String::from_utf8_lossy(&remote_add.stderr).contains("already exists")
    {
        panic!(
            "Failed to add remote: {}",
            String::from_utf8_lossy(&remote_add.stderr)
        );
    }

    let push = Command::new("git")
        .args(["push", "test", "HEAD:master"])
        .current_dir(test_repo_path)
        .output()
        .expect("git push failed");

    if !push.status.success() {
        panic!("git push failed: {}", String::from_utf8_lossy(&push.stderr));
    }

    println!("Successfully pushed to Forgejo test repo");
}

#[test]
fn test_standard_flow() {
    common::setup();
    push_to_forgejo();
}
