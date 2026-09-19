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
fn analyze_invalid_repository_returns_error_exit_code_and_diagnostic() {
    let missing = std::env::temp_dir().join(format!(
        "ferric-lens-analyze-missing-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&missing);
    let output = run(&["analyze", missing.to_str().unwrap(), "--base", "HEAD"]);

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

#[test]
fn analyze_accepts_explicit_profile_and_evidence_options() {
    let repo = Repo::baseline("analyze-options");
    let seed = ferric_lens::analyze_with_base(&repo.root, Some("HEAD")).unwrap();
    repo.write(
        "evidence.json",
        &format!(
            r#"{{"schema_version":1,"producer":{{"name":"fixture","version":"1"}},"source":{{"content_digest":"{}"}},"configuration":{{"target":"host","features":[]}},"observations":[]}}"#,
            seed.snapshot.content_digest
        ),
    );
    let json = repo.root.join("result.json");
    let html = repo.root.join("result.html");
    let output = run(&[
        "analyze",
        repo.root.to_str().unwrap(),
        "--base",
        "HEAD",
        "--target",
        &seed.profile.resolved_target,
        "--feature",
        "alpha",
        "--evidence",
        repo.root.join("evidence.json").to_str().unwrap(),
        "--json",
        json.to_str().unwrap(),
        "--html",
        html.to_str().unwrap(),
    ]);

    assert!(output.status.success());
    let value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(json).unwrap()).unwrap();
    assert_eq!(value["profile"]["features"][0], "alpha");
}

#[test]
fn analyze_reports_output_write_failures_as_errors() {
    let repo = Repo::baseline("write-error");
    let output = run(&[
        "analyze",
        repo.root.to_str().unwrap(),
        "--base",
        "HEAD",
        "--json",
        repo.root.to_str().unwrap(),
        "--html",
        repo.root.join("report.html").to_str().unwrap(),
    ]);

    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot replace"));
}

#[test]
fn regression_uses_exit_code_one_and_accept_can_succeed() {
    let (repo, baseline) = gate_fixture("cli-regression");
    let check = run(&["check", repo.root.to_str().unwrap(), "--base", &baseline]);
    assert_eq!(check.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&check.stdout).contains("Regression"));

    let result = ferric_lens::check_with_base(&repo.root, Some(&baseline)).unwrap();
    let fingerprint = result
        .findings
        .iter()
        .find(|finding| finding.gate)
        .unwrap()
        .fingerprint
        .clone();

    let accepted = run(&[
        "accept",
        &fingerprint,
        "--reason",
        "intentional CLI fixture",
        "--path",
        repo.root.to_str().unwrap(),
        "--base",
        &baseline,
    ]);

    assert!(accepted.status.success());
    assert!(String::from_utf8_lossy(&accepted.stdout).contains("accepted finding"));
}

fn gate_fixture(name: &str) -> (Repo, String) {
    let repo = Repo::new(name);
    let declarations = (0..20)
        .map(|index| format!("mod m{index};"))
        .collect::<Vec<_>>()
        .join("\n");
    repo.write("src/lib.rs", &declarations);
    for index in 0..20 {
        let (decisions, dependencies) = match index {
            0 => (12, 4),
            1..=16 => (10, 3),
            _ => (15, 5),
        };
        repo.write(
            &format!("src/m{index}.rs"),
            &gate_module_source(index, decisions, dependencies),
        );
    }
    repo.commit("baseline");
    let baseline = support::git(&repo.root, &["rev-parse", "HEAD"]);
    repo.write("src/m0.rs", &gate_module_source(0, 18, 7));
    repo.commit("regression");
    (repo, baseline)
}

fn gate_module_source(index: usize, decisions: usize, dependencies: usize) -> String {
    let mut source = String::new();
    for dependency in 1..=dependencies {
        let target = (index + dependency) % 20;
        if target != index {
            source.push_str(&format!("use crate::m{target};\n"));
        }
    }
    source.push_str("fn measured(value: bool) {\n");
    for _ in 0..decisions {
        source.push_str("    if value {}\n");
    }
    source.push_str("}\n");
    source
}
