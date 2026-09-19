//! Specification-derived regression tests for behavior that existed before
//! Ferric Lens made TDD an explicit project contract.

mod support;

use ferric_lens::{model::GateVerdict, report};
use support::Repo;

#[test]
fn no_baseline_is_explicitly_inconclusive_but_still_analyzes() {
    let repo = Repo::new("no-baseline");
    repo.write("src/lib.rs", "pub fn answer() -> usize { 42 }\n");

    let result = ferric_lens::check_with_base(&repo.root, None).unwrap();

    assert_eq!(result.verdict, GateVerdict::Inconclusive);
    assert!(result.baseline.is_none());
    assert_eq!(result.snapshot.source_files, 1);
    assert!(result
        .capabilities
        .iter()
        .any(|capability| capability.name == "baseline_comparison"));
}

#[test]
fn unchanged_repository_passes_against_an_explicit_baseline() {
    let repo = Repo::baseline("baseline");

    let result = ferric_lens::check_with_base(&repo.root, Some("HEAD")).unwrap();

    assert_eq!(result.verdict, GateVerdict::Pass);
    assert!(result.baseline.is_some());
    assert!(result.findings.iter().all(|finding| !finding.gate));
}

#[test]
fn check_and_analyze_agree_on_gate_contract() {
    let repo = Repo::baseline("baseline");

    let check = ferric_lens::check_with_base(&repo.root, Some("HEAD")).unwrap();
    let analyze = ferric_lens::analyze_with_base(&repo.root, Some("HEAD")).unwrap();

    assert_eq!(check.verdict, analyze.verdict);
    let check_gate = check
        .findings
        .iter()
        .filter(|finding| finding.gate)
        .map(|finding| (&finding.fingerprint, &finding.rule, &finding.subject))
        .collect::<Vec<_>>();
    let analyze_gate = analyze
        .findings
        .iter()
        .filter(|finding| finding.gate)
        .map(|finding| (&finding.fingerprint, &finding.rule, &finding.subject))
        .collect::<Vec<_>>();
    assert_eq!(check_gate, analyze_gate);
}

#[test]
fn outputs_are_reproducible_for_identical_inputs() {
    let repo = Repo::baseline("baseline");

    let first = ferric_lens::check_with_base(&repo.root, Some("HEAD")).unwrap();
    let second = ferric_lens::check_with_base(&repo.root, Some("HEAD")).unwrap();

    assert_eq!(
        report::json(&first),
        report::json(&second)
    );
    assert_eq!(report::html(&first), report::html(&second));
}

#[test]
fn json_and_html_share_one_semantic_result_digest() {
    let repo = Repo::baseline("baseline");
    let result = ferric_lens::check_with_base(&repo.root, Some("HEAD")).unwrap();

    let json = report::json(&result);
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    let digest = value["result_digest"].as_str().unwrap();

    assert_eq!(digest, report::result_digest(&result));
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
    assert!(result
        .capabilities
        .iter()
        .any(|capability| capability.name == "head.syntax_extraction"));
    assert!(result.modules.iter().any(|module| !module.parse_complete));
}

#[test]
fn explicit_feature_selection_is_normalized_and_recorded() {
    let repo = Repo::baseline("baseline");
    let features = vec!["zeta".to_owned(), "alpha".to_owned(), "alpha".to_owned()];

    let result =
        ferric_lens::check_with_profile(&repo.root, Some("HEAD"), None, &features).unwrap();

    assert_eq!(result.profile.features, vec!["alpha", "zeta"]);
    assert!(result.profile.id.contains("alpha,zeta"));
}

#[test]
fn imported_evidence_is_optional_and_does_not_change_the_gate_verdict() {
    let repo = Repo::baseline("baseline");
    let without = ferric_lens::analyze_with_base(&repo.root, Some("HEAD")).unwrap();

    let evidence_path = repo.root.join("evidence.json");
    repo.write(
        "evidence.json",
        &format!(
            r#"{{
  "schema_version": 1,
  "producer": {{"name": "fixture", "version": "1"}},
  "source": {{"content_digest": "{}"}},
  "configuration": {{"target": "host", "features": []}},
  "observations": [
    {{"subject": "src/lib.rs", "metric": "instructions", "value": 7, "unit": "count"}}
  ]
}}"#,
            without.snapshot.content_digest
        ),
    );

    let with =
        ferric_lens::analyze_with_base_and_evidence(&repo.root, Some("HEAD"), Some(&evidence_path))
            .unwrap();

    assert_eq!(without.verdict, with.verdict);
    assert!(with.imported_evidence.as_ref().unwrap().attached);
}

#[test]
fn accept_rejects_unknown_finding_fingerprints() {
    let repo = Repo::baseline("baseline");

    let error = ferric_lens::accept_finding_with_base(
        &repo.root,
        Some("HEAD"),
        "not-a-current-finding",
        "documented exception",
    )
    .unwrap_err();

    assert!(error.contains("not present in the current analysis"));
}

#[test]
fn analyze_wrapper_returns_a_useful_inconclusive_report_without_an_automatic_baseline() {
    let repo = Repo::baseline("analyze-wrapper");

    let result = ferric_lens::analyze(&repo.root).unwrap();

    assert_eq!(
        result.verdict,
        ferric_lens::model::GateVerdict::Inconclusive
    );
    assert!(!result.modules.is_empty());
}

#[test]
fn full_analysis_enriches_changed_production_paths_with_history() {
    let repo = Repo::baseline("history-enrichment");
    repo.write(
        "src/stable.rs",
        "pub fn stable() -> usize { if true { 2 } else { 1 } }",
    );

    let result = ferric_lens::analyze_with_base(&repo.root, Some("HEAD")).unwrap();

    let history = result
        .history
        .as_ref()
        .expect("history should be collected");
    assert!(history.sampled_commits >= 1);
    let module = result
        .modules
        .iter()
        .find(|module| module.path == "src/stable.rs")
        .unwrap();
    assert!(module.history.is_some());
}
