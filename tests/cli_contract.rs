mod support;

use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

use support::Repo;

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ferric-lens"))
}

fn run(args: &[&str]) -> Output {
    Command::new(binary()).args(args).output().unwrap()
}

#[test]
fn check_passes_and_can_write_json() {
    let repo = Repo::baseline("check");
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
    let repo = Repo::baseline("analyze");
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
    let repo = Repo::baseline("inconclusive");
    let output = run(&["check", repo.root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stdout).contains("Inconclusive"));
}

#[test]
fn invalid_repository_returns_error_exit_code_and_diagnostic() {
    let missing = std::env::temp_dir().join(format!("ferric-lens-missing-{}", std::process::id()));
    let _ = fs::remove_dir_all(&missing);
    let output = run(&["check", missing.to_str().unwrap(), "--base", "HEAD"]);

    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("ferric-lens:"));
}

#[test]
fn accept_rejects_unknown_fingerprint_through_cli() {
    let repo = Repo::baseline("accept");
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
    assert!(String::from_utf8_lossy(&output.stderr).contains("not present in the current analysis"));
}
