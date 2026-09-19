use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

use crate::{
    compare::Correspondence,
    git::{ChangeSet, HistoryCommit, HistorySample},
    model::{CapabilityStatus, DeltaStatus, EvidenceClass, Finding, ModuleMetrics, Priority},
};

fn module(crate_name: &str, module_path: &str, path: &str) -> ModuleMetrics {
    ModuleMetrics {
        crate_name: crate_name.into(),
        module_path: module_path.into(),
        path: path.into(),
        lines: 1,
        decision_sites: 0,
        public_items: 0,
        explicit_imports: Vec::new(),
        local_dependency_modules: Vec::new(),
        structure_digest: path.into(),
        parse_complete: true,
        gate_complete: true,
        limitation: None,
        history: None,
    }
}

fn finding(rule: &str, subject: &str) -> Finding {
    Finding {
        fingerprint: String::new(),
        rule: rule.into(),
        subject: subject.into(),
        identity: subject.into(),
        configuration: String::new(),
        evidence_class: EvidenceClass::Candidate,
        priority: Priority::Observe,
        delta: DeltaStatus::Current,
        gate: false,
        accepted: false,
        acceptance_reason: None,
        summary: String::new(),
        direction: String::new(),
        evidence: Vec::new(),
    }
}

#[test]
fn history_candidates_include_changed_renamed_and_finding_paths_only_when_owned() {
    let modules = vec![
        module("demo", "", "src/lib.rs"),
        module("demo", "a", "src/a.rs"),
        module("demo", "b", "src/b.rs"),
    ];
    let changes = ChangeSet {
        added: BTreeSet::from(["src/a.rs".into(), "README.md".into()]),
        modified: BTreeSet::from(["src/lib.rs".into()]),
        deleted: BTreeSet::new(),
        renames: BTreeMap::from([("src/old.rs".into(), "src/b.rs".into())]),
    };
    let findings = vec![finding("advisory", "demo::a"), finding("other", "missing")];

    let candidates = super::history_candidates(&modules, &changes, &findings);

    assert_eq!(
        candidates,
        BTreeSet::from([
            "src/a.rs".to_owned(),
            "src/b.rs".to_owned(),
            "src/lib.rs".to_owned(),
        ])
    );
}

#[test]
fn advisory_identity_rebinds_only_with_matching_subject_and_correspondence() {
    let head = vec![module("demo", "moved", "src/moved.rs")];
    let baseline = vec![module("demo", "old", "src/old.rs")];
    let mut findings = vec![
        finding("structure.current_coupled_outlier", "demo::moved"),
        finding("structure.current_coupled_outlier", "missing"),
        finding("other", "demo::moved"),
    ];
    let correspondence = Correspondence {
        head_to_baseline: [(0, 0)].into_iter().collect(),
        ambiguous_head: Default::default(),
    };

    super::rebind_advisory_identities(&mut findings, &head, &baseline, &correspondence);

    assert_eq!(findings[0].identity, "demo::old");
    assert_eq!(findings[1].identity, "missing");
    assert_eq!(findings[2].identity, "demo::moved");

    let mut no_match = vec![finding("structure.current_coupled_outlier", "demo::moved")];
    super::rebind_advisory_identities(&mut no_match, &head, &baseline, &Correspondence::default());
    assert_eq!(no_match[0].identity, "demo::moved");
}

#[test]
fn display_subject_and_short_oid_cover_root_and_short_values() {
    assert_eq!(super::display_subject("demo", ""), "demo");
    assert_eq!(super::display_subject("demo", "a"), "demo::a");
    assert_eq!(super::short_oid("1234567890123456"), "123456789012");
    assert_eq!(super::short_oid("short"), "short");
}

#[test]
fn snapshot_capabilities_reflect_complete_and_partial_syntax_and_inventory() {
    let complete = super::SnapshotAnalysis {
        content_digest: "digest".into(),
        metadata_complete: true,
        metadata_detail: None,
        modules: Vec::new(),
        parse_failures: 0,
    };
    let complete_caps = super::snapshot_capabilities("head", &complete);
    assert!(complete_caps
        .iter()
        .all(|capability| capability.status == CapabilityStatus::Complete));

    let partial = super::SnapshotAnalysis {
        content_digest: "digest".into(),
        metadata_complete: false,
        metadata_detail: Some("metadata unavailable".into()),
        modules: Vec::new(),
        parse_failures: 2,
    };
    let partial_caps = super::snapshot_capabilities("baseline", &partial);
    assert!(partial_caps
        .iter()
        .all(|capability| capability.status == CapabilityStatus::Partial));
    assert!(partial_caps
        .iter()
        .any(|capability| capability.detail.as_deref()
            == Some("2 Rust source file(s) could not be parsed")));
}
static REPO_COUNTER: AtomicU64 = AtomicU64::new(0);

struct Repo {
    root: PathBuf,
}

impl Repo {
    fn new(name: &str) -> Self {
        let counter = REPO_COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "ferric-lens-lib-test-{}-{counter}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname='demo'\nversion='0.1.0'\nedition='2021'\n",
        )
        .unwrap();
        fs::write(root.join("src/lib.rs"), "pub fn stable() -> usize { 1 }\n").unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        git(&root, &["config", "user.email", "test@example.invalid"]);
        git(&root, &["config", "user.name", "Ferric Lens Test"]);
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-q", "-m", "baseline"]);
        Self { root }
    }

    fn write(&self, path: &str, contents: &str) {
        let path = self.root.join(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn baseline_failure(
    _: &Path,
    _: &Path,
    _: &crate::profile::ProfileContext,
) -> Result<super::SnapshotAnalysis, String> {
    Err("fixture baseline analysis failure".into())
}

fn history_failure(_: &Path) -> Result<HistorySample, String> {
    Err("fixture history failure".into())
}

fn truncated_history(_: &Path) -> Result<HistorySample, String> {
    Ok(HistorySample {
        commits: vec![HistoryCommit {
            oid: "a".repeat(40),
            paths: vec!["src/lib.rs".into()],
        }],
        changed_path_records: 1,
        broad_commits_excluded_from_cochange: 0,
        truncated: true,
    })
}

#[test]
fn baseline_materialization_failure_is_reported_as_inconclusive() {
    let repo = Repo::new("materialize-failure");
    fs::write(
        repo.root.join(".git/worktrees"),
        "blocks worktree directory",
    )
    .unwrap();

    let result = super::analyze_with_base(&repo.root, Some("HEAD")).unwrap();

    assert_eq!(result.verdict, crate::model::GateVerdict::Inconclusive);
    assert!(result
        .verdict_reason
        .contains("baseline materialization unavailable"));
}

#[test]
fn baseline_analysis_failure_is_reported_as_inconclusive() {
    let repo = Repo::new("baseline-analysis-failure");

    let ops = super::AnalysisOps {
        materialize_baseline: crate::git::materialize_worktree,
        analyze_baseline: baseline_failure,
        sample_history: crate::git::sample_history,
    };
    let result =
        super::analyze_internal_with_ops(&repo.root, Some("HEAD"), false, None, None, &[], &ops)
            .unwrap();

    assert_eq!(result.verdict, crate::model::GateVerdict::Inconclusive);
    assert!(result.verdict_reason.contains("baseline analysis failed"));
}

#[test]
fn unavailable_history_degrades_capability_without_changing_gate_semantics() {
    let repo = Repo::new("history-failure");
    repo.write("src/lib.rs", "pub fn stable() -> usize { 2 }\n");

    let ops = super::AnalysisOps {
        materialize_baseline: crate::git::materialize_worktree,
        analyze_baseline: super::analyze_snapshot,
        sample_history: history_failure,
    };
    let result =
        super::analyze_internal_with_ops(&repo.root, Some("HEAD"), true, None, None, &[], &ops)
            .unwrap();

    assert!(result.history.is_none());
    let capability = result
        .capabilities
        .iter()
        .find(|capability| capability.name == "history_enrichment")
        .unwrap();
    assert_eq!(capability.status, CapabilityStatus::Unavailable);
    assert_eq!(
        capability.detail.as_deref(),
        Some("fixture history failure")
    );
}

#[test]
fn truncated_history_is_explicitly_partial() {
    let repo = Repo::new("history-truncated");
    repo.write("src/lib.rs", "pub fn stable() -> usize { 2 }\n");

    let ops = super::AnalysisOps {
        materialize_baseline: crate::git::materialize_worktree,
        analyze_baseline: super::analyze_snapshot,
        sample_history: truncated_history,
    };
    let result =
        super::analyze_internal_with_ops(&repo.root, Some("HEAD"), true, None, None, &[], &ops)
            .unwrap();

    let history = result.history.as_ref().unwrap();
    assert!(history.truncated);
    let capability = result
        .capabilities
        .iter()
        .find(|capability| capability.name == "history_enrichment")
        .unwrap();
    assert_eq!(capability.status, CapabilityStatus::Partial);
    assert!(capability
        .detail
        .as_deref()
        .unwrap()
        .contains("sample truncated"));
}\n
#[test]
fn finding_sort_is_gate_first_then_rule_and_subject() {
    let mut findings = vec![
        finding("z", "demo::z"),
        finding("a", "demo::b"),
        finding("a", "demo::a"),
    ];
    findings[1].gate = true;
    findings[2].gate = true;

    super::sort_findings(&mut findings);

    assert_eq!(
        findings
            .iter()
            .map(|finding| (finding.gate, finding.rule.as_str(), finding.subject.as_str()))
            .collect::<Vec<_>>(),
        vec![
            (true, "a", "demo::a"),
            (true, "a", "demo::b"),
            (false, "z", "demo::z"),
        ]
    );
}
