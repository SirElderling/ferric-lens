//! Specification-derived regression tests for behavior that existed before
//! Ferric Lens made TDD an explicit project contract.

use std::{\n    fs,\n    path::{Path, PathBuf},\n    process::Command,\n    sync::atomic::{AtomicU64, Ordering},\n};
use ferric_lens::{model::GateVerdict, report};

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct Repo {\n    root: PathBuf,\n}

impl Repo {
    fn new(name: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("ferric-lens-contract-{name}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("Cargo.toml"), "[package]\nname='fixture'\nversion='0.1.0'\nedition='2021'\n").unwrap();
        git(&root, &["init", "-q"]);
        git(&root, &["config", "user.email", "ferric-lens@example.invalid"]);
        git(&root, &["config", "user.name", "Ferric Lens Test"]);
        Self { root }
    }
    fn write(&self, path: &str, contents: &str) {
        let path = self.root.join(path);
        if let Some(parent) = path.parent() {\n            fs::create_dir_all(parent).unwrap();\n        }
        fs::write(path, contents).unwrap();
    }
    fn commit(&self, message: &str) {
        git(&self.root, &["add", "."]);
        git(&self.root, &["commit", "-q", "-m", message]);
    }
}
impl Drop for Repo {\n    fn drop(&mut self) {\n        let _ = fs::remove_dir_all(&self.root);\n    }\n}

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git").current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1").env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args(args).output().unwrap();
    assert!(output.status.success(), "git {:?} failed: {}", args, String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}
fn baseline_repo() -> Repo {
    let repo = Repo::new("baseline");
    repo.write("src/lib.rs", "pub mod stable;\n");
    repo.write("src/stable.rs", "pub fn stable() -> usize { 1 }\n");
    repo.commit("baseline");
    repo
}

#[test]
fn no_baseline_is_explicitly_inconclusive_but_still_analyzes() {
    let repo = Repo::new("no-baseline");
    repo.write("src/lib.rs", "pub fn answer() -> usize { 42 }\n");
    let result = ferric_lens::check_with_base(&repo.root, None).unwrap();
    assert_eq!(result.verdict, GateVerdict::Inconclusive);
    assert!(result.baseline.is_none());
    assert_eq!(result.snapshot.source_files, 1);
    assert!(result.capabilities.iter().any(|c| c.name == "baseline_comparison"));
}

#[test]
fn unchanged_repository_passes_against_an_explicit_baseline() {
    let repo = baseline_repo();
    let result = ferric_lens::check_with_base(&repo.root, Some("HEAD")).unwrap();
    assert_eq!(result.verdict, GateVerdict::Pass);
    assert!(result.baseline.is_some());
    assert!(result.findings.iter().all(|f| !f.gate));
}

#[test]
fn check_and_analyze_agree_on_gate_contract() {
    let repo = baseline_repo();
    let check = ferric_lens::check_with_base(&repo.root, Some("HEAD")).unwrap();
    let analyze = ferric_lens::analyze_with_base(&repo.root, Some("HEAD")).unwrap();
    assert_eq!(check.verdict, analyze.verdict);
    let check_gate = check.findings.iter().filter(|f| f.gate).map(|f| (&f.fingerprint, &f.rule, &f.subject)).collect::<Vec<_>>();
    let analyze_gate = analyze.findings.iter().filter(|f| f.gate).map(|f| (&f.fingerprint, &f.rule, &f.subject)).collect::<Vec<_>>();
    assert_eq!(check_gate, analyze_gate);
}

#[test]
fn outputs_are_reproducible_for_identical_inputs() {
    let repo = baseline_repo();
    let first = ferric_lens::check_with_base(&repo.root, Some("HEAD")).unwrap();
    let second = ferric_lens::check_with_base(&repo.root, Some("HEAD")).unwrap();
    assert_eq!(\n        report::json(&first).unwrap(),\n        report::json(&second).unwrap()\n    );
    assert_eq!(report::html(&first), report::html(&second));
}

#[test]
fn json_and_html_share_one_semantic_result_digest() {
    let repo = baseline_repo();
    let result = ferric_lens::check_with_base(&repo.root, Some("HEAD")).unwrap();
    let value: serde_json::Value = serde_json::from_str(&report::json(&result).unwrap()).unwrap();
    let digest = value["result_digest"].as_str().unwrap();
    assert_eq!(digest, report::result_digest(&result).unwrap());
    assert!(report::html(&result).contains(digest));
}

#[test]
fn auxiliary_test_sources_do_not_enter_the_production_snapshot() {
    let repo = Repo::new("source-scope");
    repo.write("src/lib.rs", "pub fn production() {}\n");
    repo.write("tests/integration.rs", "#[test] fn integration() {}\n");
    repo.write("examples/demo.rs", "fn main() {}\n");
    repo.write("benches/bench.rs", "fn main() {}\n");
    repo.commit("baseline");
    let result = ferric_lens::check_with_base(&repo.root, Some("HEAD")).unwrap();
    assert_eq!(result.snapshot.source_files, 1);
    assert_eq!(result.modules[0].path, "src/lib.rs");
}

#[test]
fn malformed_rust_is_reported_as_incomplete_not_silently_complete() {
    let repo = Repo::new("partial");
    repo.write("src/lib.rs", "pub fn broken( {\n");
    repo.commit("baseline");
    let result = ferric_lens::check_with_base(&repo.root, Some("HEAD")).unwrap();
    assert_eq!(result.verdict, GateVerdict::Inconclusive);
    assert!(result\n        .capabilities\n        .iter()\n        .any(|c| c.name == "head.syntax_extraction"));
    assert!(result.modules.iter().any(|m| !m.parse_complete));
}

#[test]
fn explicit_feature_selection_is_normalized_and_recorded() {
    let repo = baseline_repo();
    let features = vec!["zeta".to_owned(), "alpha".to_owned(), "alpha".to_owned()];
    let result =\n        ferric_lens::check_with_profile(&repo.root, Some("HEAD"), None, &features).unwrap();
    assert_eq!(result.profile.features, vec!["alpha", "zeta"]);
    assert!(result.profile.id.contains("alpha,zeta"));
}

#[test]
fn imported_evidence_is_optional_and_does_not_change_the_gate_verdict() {
    let repo = baseline_repo();
    let without = ferric_lens::analyze_with_base(&repo.root, Some("HEAD")).unwrap();
    let evidence_path = repo.root.join("evidence.json");
    repo.write("evidence.json", &format!(r#"{{
  "schema_version": 1,
  "producer": {{"name": "fixture", "version": "1"}},
  "source": {{"content_digest": "{}"}},
  "configuration": {{"target": "host", "features": []}},
  "observations": [{{"subject": "src/lib.rs", "metric": "instructions", "value": 7, "unit": "count"}}]
}}"#, without.snapshot.content_digest));
    let with =\n        ferric_lens::analyze_with_base_and_evidence(&repo.root, Some("HEAD"), Some(&evidence_path))\n            .unwrap();
    assert_eq!(without.verdict, with.verdict);
    assert!(with.imported_evidence.as_ref().unwrap().attached);
}

#[test]
fn accept_rejects_unknown_finding_fingerprints() {
    let repo = baseline_repo();
    let error = ferric_lens::accept_finding_with_base(\n        &repo.root,\n        Some("HEAD"),\n        "not-a-current-finding",\n        "documented exception",\n    )\n    .unwrap_err();
    assert!(error.contains("not present in the current analysis"));
}
