use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct Repo {
    root: PathBuf,
}

impl Repo {
    fn new(name: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "ferric-lens-cli-{name}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname='fixture'\nversion='0.1.0'\nedition='2021'\n",
        )
        .unwrap();
        fs::write(root.join("src/lib.rs"), "pub fn stable() -> usize { 1 }\n").unwrap();
        git(&root, &["init", "-q"]);
        git(&root, &["config", "user.email", "ferric-lens@example.invalid"]);
        git(&root, &["config", "user.name", "Ferric Lens Test"]);
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "baseline"]);
        Self { root }
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success());
}

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ferric-lens"))
}

fn run(args: &[&str]) -> Output {
    Command::new(binary()).args(args).output().unwrap()
}

#[test]
fn check_passes_and_can_write_json() {
    let repo = Repo::new("check");
    let json = repo.root.join("out/check.json");
    let output = run(&[
        "check",
        repo.root.to_str().unwrap(),
        "--base",
        "HEAD",
        "--json",
        json.to_str().unwrap(),
    ]);

    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("Ferric Lens: Pass"));
    let value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(json).unwrap()).unwrap();
    assert_eq!(value["verdict"], "pass");
    assert!(value["result_digest"].is_string());
}

#[test]
fn analyze_writes_json_and_html_with_matching_digest() {
    let repo = Repo::new("analyze");
    let json = repo.root.join("artifacts/result.json");
    let html = repo.root.join("artifacts/result.html");
    let output = run(&[
        "analyze",
        repo.root.to_str().unwrap(),
        "--base",
        "HEAD",
        "--json",
        json.to_str().unwrap(),
        "--html",
        html.to_str().unwrap(),
    ]);

    assert!(output.status.success());
    let value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&json).unwrap()).unwrap();
    let digest = value["result_digest"].as_str().unwrap();
    assert!(fs::read_to_string(html).unwrap().contains(digest));
}

#[test]
fn missing_baseline_returns_inconclusive_exit_code() {
    let repo = Repo::new("inconclusive");
    let output = run(&["check", repo.root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stdout).contains("Inconclusive"));
}

#[test]
fn invalid_repository_returns_error_exit_code_and_diagnostic() {
    let missing = std::env::temp_dir().join(format!(
        "ferric-lens-missing-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let output = run(&["check", missing.to_str().unwrap(), "--base", "HEAD"]);

    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("ferric-lens:"));
}

#[test]
fn accept_rejects_unknown_fingerprint_through_cli() {
    let repo = Repo::new("accept");
    let output = run(&[
        "accept",
        "not-a-current-finding",
        "--reason",
        "fixture",
        "--path",
        repo.root.to_str().unwrap(),
        "--base",
        "HEAD",
    ]);

    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("not present in the current analysis"));
}
