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
    model::{
        CapabilityStatus, DeltaStatus, Evidence, EvidenceClass, Finding, ModuleMetrics, Priority,
        SourceContext,
    },
};

fn module(crate_name: &str, module_path: &str, path: &str) -> ModuleMetrics {
    ModuleMetrics {
        crate_name: crate_name.into(),
        module_path: module_path.into(),
        path: path.into(),
        lines: 1,
        decision_sites: 0,
        public_items: 0,
        clone_calls: 0,
        functions: Vec::new(),
        types: Vec::new(),
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
        cargo_input_digest: "cargo-input".into(),
        cargo_resolution_digest: Some("cargo-resolution".into()),
        workspace_aliases: Default::default(),
        resolved_features_by_crate: Default::default(),
        source_digests: Default::default(),
        auxiliary_targets: crate::input::AuxiliaryTargetSummary {
            tests: 1,
            benches: 2,
            examples: 3,
            build_scripts: 4,
            proc_macros: 5,
        },
        modules: Vec::new(),
        correctness_findings: Vec::new(),
        correctness_contexts: Vec::new(),
        parse_failures: 0,
    };
    let complete_caps = super::snapshot_capabilities("head", &complete);
    assert!(complete_caps
        .iter()
        .all(|capability| capability.status == CapabilityStatus::Complete));
    let resolution = complete_caps
        .iter()
        .find(|capability| capability.name == "head.cargo_resolution")
        .unwrap();
    assert!(resolution
        .detail
        .as_deref()
        .unwrap()
        .contains("cargo-resolution"));
    let scopes = complete_caps
        .iter()
        .find(|capability| capability.name == "head.source_scopes")
        .unwrap();
    assert!(scopes
        .detail
        .as_deref()
        .unwrap()
        .contains("tests=1, benches=2, examples=3, build_scripts=4, proc_macros=5"));

    let partial = super::SnapshotAnalysis {
        content_digest: "digest".into(),
        metadata_complete: false,
        metadata_detail: Some("metadata unavailable".into()),
        cargo_input_digest: "fallback-cargo-input".into(),
        cargo_resolution_digest: None,
        workspace_aliases: Default::default(),
        resolved_features_by_crate: Default::default(),
        source_digests: Default::default(),
        auxiliary_targets: crate::input::AuxiliaryTargetSummary::default(),
        modules: Vec::new(),
        correctness_findings: Vec::new(),
        correctness_contexts: Vec::new(),
        parse_failures: 2,
    };
    let partial_caps = super::snapshot_capabilities("baseline", &partial);
    assert!(partial_caps.iter().any(|capability| {
        capability.name == "baseline.source_inventory"
            && capability.status == CapabilityStatus::Partial
    }));
    assert!(partial_caps.iter().any(|capability| {
        capability.name == "baseline.cargo_resolution"
            && capability.status == CapabilityStatus::Unavailable
    }));
    assert!(partial_caps.iter().any(|capability| {
        capability.name == "baseline.source_scopes"
            && capability.status == CapabilityStatus::Unavailable
    }));
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
        fs::write(
            root.join("Cargo.lock"),
            "version = 4\n\n[[package]]\nname = \"demo\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
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

#[test]
fn empty_unanalyzable_baseline_is_inconclusive_not_pass() {
    let repo = Repo::new("empty-baseline");
    fs::remove_file(repo.root.join("Cargo.toml")).unwrap();
    fs::remove_dir_all(repo.root.join("src")).unwrap();
    repo.write("README.md", "baseline without Rust");
    git(&repo.root, &["add", "-A"]);
    git(&repo.root, &["commit", "-q", "-m", "empty baseline"]);
    let baseline = git(&repo.root, &["rev-parse", "HEAD"]);

    repo.write(
        "Cargo.toml",
        "[package]\nname='demo'\nversion='0.1.0'\nedition='2021'\n",
    );
    repo.write("src/lib.rs", "pub fn current() -> usize { 1 }\n");

    let result = super::analyze_with_base(&repo.root, Some(&baseline)).unwrap();

    assert_eq!(result.verdict, crate::model::GateVerdict::Inconclusive);
    assert!(result
        .verdict_reason
        .contains("baseline Cargo/source inventory is incomplete"));
    let comparison = result
        .capabilities
        .iter()
        .find(|capability| capability.name == "baseline_comparison")
        .unwrap();
    assert_eq!(comparison.status, CapabilityStatus::Partial);
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
        source_contexts: super::finding_source_contexts,
    };
    let result =
        super::analyze_internal_with_ops(&repo.root, Some("HEAD"), false, None, None, &[], &ops)
            .unwrap();

    assert_eq!(result.verdict, crate::model::GateVerdict::Inconclusive);
    assert!(result.verdict_reason.contains("baseline analysis failed"));
}

fn source_context_failure(
    _: &Path,
    _: &super::SnapshotAnalysis,
    _: &[Finding],
    _: &crate::profile::ProfileContext,
) -> Result<Vec<crate::model::SourceContext>, String> {
    Err("fixture source context failure".into())
}

#[test]
fn analysis_propagates_source_context_failures_from_all_result_paths() {
    let invalid_base = Repo::new("source-context-baseline-resolution");
    let ops = super::AnalysisOps {
        materialize_baseline: crate::git::materialize_worktree,
        analyze_baseline: super::analyze_snapshot,
        sample_history: crate::git::sample_history,
        source_contexts: source_context_failure,
    };
    assert!(super::analyze_internal_with_ops(
        &invalid_base.root,
        Some("missing-baseline"),
        false,
        None,
        None,
        &[],
        &ops,
    )
    .unwrap_err()
    .contains("fixture source context failure"));

    let materialize = Repo::new("source-context-materialize");
    fs::write(
        materialize.root.join(".git/worktrees"),
        "blocks worktree directory",
    )
    .unwrap();
    assert!(super::analyze_internal_with_ops(
        &materialize.root,
        Some("HEAD"),
        false,
        None,
        None,
        &[],
        &ops,
    )
    .unwrap_err()
    .contains("fixture source context failure"));

    let baseline = Repo::new("source-context-baseline-analysis");
    let baseline_ops = super::AnalysisOps {
        materialize_baseline: crate::git::materialize_worktree,
        analyze_baseline: baseline_failure,
        sample_history: crate::git::sample_history,
        source_contexts: source_context_failure,
    };
    assert!(super::analyze_internal_with_ops(
        &baseline.root,
        Some("HEAD"),
        false,
        None,
        None,
        &[],
        &baseline_ops,
    )
    .unwrap_err()
    .contains("fixture source context failure"));

    let final_result = Repo::new("source-context-final-result");
    assert!(super::analyze_internal_with_ops(
        &final_result.root,
        Some("HEAD"),
        false,
        None,
        None,
        &[],
        &ops,
    )
    .unwrap_err()
    .contains("fixture source context failure"));
}

#[test]
fn finding_source_contexts_merges_only_requested_correctness_contexts() {
    let repo = Repo::new("correctness-context-merge");
    let profile = crate::profile::ProfileContext::resolve(None, &[]).unwrap();
    let mut wanted = finding("correctness.test", "repository");
    wanted.evidence = vec![Evidence {
        metric: "wanted".into(),
        value: 1,
        reference: 0,
        population: 1,
        baseline: None,
        material_delta: None,
    }];
    let snapshot = super::SnapshotAnalysis {
        content_digest: "digest".into(),
        metadata_complete: true,
        metadata_detail: None,
        cargo_input_digest: "cargo".into(),
        cargo_resolution_digest: Some("resolution".into()),
        auxiliary_targets: Default::default(),
        workspace_aliases: Default::default(),
        resolved_features_by_crate: Default::default(),
        source_digests: Default::default(),
        modules: Vec::new(),
        correctness_findings: Vec::new(),
        correctness_contexts: vec![
            SourceContext {
                subject: "repository".into(),
                metric: "wanted".into(),
                path: "src/a.rs".into(),
                start_line: 1,
                end_line: 1,
                excerpt: "wanted".into(),
                excerpt_truncated: false,
            },
            SourceContext {
                subject: "repository".into(),
                metric: "other".into(),
                path: "src/b.rs".into(),
                start_line: 1,
                end_line: 1,
                excerpt: "other".into(),
                excerpt_truncated: false,
            },
        ],
        parse_failures: 0,
    };

    let contexts =
        super::finding_source_contexts(&repo.root, &snapshot, &[wanted], &profile).unwrap();

    assert_eq!(contexts.len(), 1);
    assert_eq!(contexts[0].metric, "wanted");
}

#[test]
fn finding_source_contexts_propagates_standard_context_read_errors() {
    let repo = Repo::new("standard-context-read-error");
    let profile = crate::profile::ProfileContext::resolve(None, &[]).unwrap();
    let mut wanted = finding("structure.test", "demo::missing");
    wanted.evidence = vec![Evidence {
        metric: "decision_sites".into(),
        value: 1,
        reference: 0,
        population: 1,
        baseline: None,
        material_delta: None,
    }];
    let missing = module("demo", "missing", "src/missing.rs");
    let snapshot = super::SnapshotAnalysis {
        content_digest: "digest".into(),
        metadata_complete: true,
        metadata_detail: None,
        cargo_input_digest: "cargo".into(),
        cargo_resolution_digest: Some("resolution".into()),
        auxiliary_targets: Default::default(),
        workspace_aliases: Default::default(),
        resolved_features_by_crate: Default::default(),
        source_digests: BTreeMap::from([("src/missing.rs".into(), "digest".into())]),
        modules: vec![missing],
        correctness_findings: Vec::new(),
        correctness_contexts: Vec::new(),
        parse_failures: 0,
    };

    let error =
        super::finding_source_contexts(&repo.root, &snapshot, &[wanted], &profile).unwrap_err();

    assert!(error.contains("cannot read"));
}

#[test]
fn unavailable_history_degrades_capability_without_changing_gate_semantics() {
    let repo = Repo::new("history-failure");
    repo.write("src/lib.rs", "pub fn stable() -> usize { 2 }\n");

    let ops = super::AnalysisOps {
        materialize_baseline: crate::git::materialize_worktree,
        analyze_baseline: super::analyze_snapshot,
        sample_history: history_failure,
        source_contexts: super::finding_source_contexts,
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
        source_contexts: super::finding_source_contexts,
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
}

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
            .map(|finding| (
                finding.gate,
                finding.rule.as_str(),
                finding.subject.as_str()
            ))
            .collect::<Vec<_>>(),
        vec![
            (true, "a", "demo::a"),
            (true, "a", "demo::b"),
            (false, "z", "demo::z"),
        ]
    );
}

#[test]
fn public_analysis_wrappers_delegate_with_consistent_semantics() {
    let repo = Repo::new("public-wrappers");

    assert_eq!(
        super::analyze(&repo.root).unwrap().verdict,
        crate::model::GateVerdict::Inconclusive
    );

    let results = [
        super::analyze_with_base(&repo.root, Some("HEAD")).unwrap(),
        super::analyze_with_base_and_evidence(&repo.root, Some("HEAD"), None).unwrap(),
        super::analyze_with_profile(&repo.root, Some("HEAD"), None, &[], None).unwrap(),
        super::check_with_base(&repo.root, Some("HEAD")).unwrap(),
        super::check_with_profile(&repo.root, Some("HEAD"), None, &[]).unwrap(),
    ];
    assert!(results
        .iter()
        .all(|result| result.verdict == crate::model::GateVerdict::Pass));

    assert!(
        super::accept_finding_with_base(&repo.root, Some("HEAD"), "missing", "not present")
            .unwrap_err()
            .contains("is not present")
    );
    assert!(super::accept_finding_with_profile(
        &repo.root,
        Some("HEAD"),
        None,
        &[],
        "missing",
        "not present"
    )
    .unwrap_err()
    .contains("is not present"));
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

#[test]
fn public_acceptance_wrappers_record_an_exact_current_finding() {
    let repo = Repo::new("public-acceptance");
    repo.write(
        "src/lib.rs",
        &(0..20)
            .map(|index| format!("mod m{index};"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
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
    git(&repo.root, &["add", "-A"]);
    git(&repo.root, &["commit", "-q", "-m", "gate baseline"]);
    let baseline = git(&repo.root, &["rev-parse", "HEAD"]);

    repo.write("src/m0.rs", &gate_module_source(0, 18, 7));
    let first = super::check_with_base(&repo.root, Some(&baseline)).unwrap();
    let fingerprint = first
        .findings
        .iter()
        .find(|finding| finding.gate)
        .unwrap()
        .fingerprint
        .clone();

    let repeated = super::check_with_base(&repo.root, Some(&baseline)).unwrap();
    assert!(
        repeated
            .findings
            .iter()
            .any(|finding| finding.fingerprint == fingerprint),
        "repeated analysis lost fingerprint {fingerprint}; findings: {:?}",
        repeated
            .findings
            .iter()
            .map(|finding| (&finding.subject, &finding.fingerprint, finding.gate))
            .collect::<Vec<_>>()
    );

    super::accept_finding_with_base(
        &repo.root,
        Some(&baseline),
        &fingerprint,
        "intentional unit boundary",
    )
    .unwrap();
    super::accept_finding_with_profile(
        &repo.root,
        Some(&baseline),
        None,
        &[],
        &fingerprint,
        "intentional unit boundary via profile",
    )
    .unwrap();

    let accepted = super::check_with_base(&repo.root, Some(&baseline)).unwrap();
    let finding = accepted
        .findings
        .iter()
        .find(|finding| finding.fingerprint == fingerprint)
        .unwrap();
    assert!(finding.accepted);
    assert_eq!(
        finding.acceptance_reason.as_deref(),
        Some("intentional unit boundary via profile")
    );
}

fn baseline_then_break_git(
    baseline_root: &Path,
    repo_root: &Path,
    profile: &crate::profile::ProfileContext,
) -> Result<super::SnapshotAnalysis, String> {
    let snapshot = super::analyze_snapshot(baseline_root, repo_root, profile)?;
    fs::rename(repo_root.join(".git"), repo_root.join(".git-disabled"))
        .map_err(|error| error.to_string())?;
    Ok(snapshot)
}

fn baseline_then_corrupt_acceptance(
    baseline_root: &Path,
    repo_root: &Path,
    profile: &crate::profile::ProfileContext,
) -> Result<super::SnapshotAnalysis, String> {
    let snapshot = super::analyze_snapshot(baseline_root, repo_root, profile)?;
    let directory = repo_root.join(".ferric-lens");
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    fs::write(directory.join("acceptances.toml"), "not = [valid")
        .map_err(|error| error.to_string())?;
    Ok(snapshot)
}

#[test]
fn analysis_propagates_profile_evidence_and_initial_acceptance_errors() {
    let repo = Repo::new("analysis-propagation");

    assert!(super::analyze_internal_with_ops(
        &repo.root,
        Some("HEAD"),
        false,
        None,
        Some("ferric-lens-invalid-target"),
        &[],
        &super::REAL_OPS,
    )
    .is_err());

    let evidence = repo.root.join("bad-evidence.json");
    fs::write(&evidence, "{").unwrap();
    assert!(super::analyze_internal_with_ops(
        &repo.root,
        Some("HEAD"),
        false,
        Some(&evidence),
        None,
        &[],
        &super::REAL_OPS,
    )
    .is_err());

    fs::create_dir_all(repo.root.join(".ferric-lens")).unwrap();
    fs::write(
        repo.root.join(".ferric-lens/acceptances.toml"),
        "not = [valid",
    )
    .unwrap();
    assert!(super::analyze_internal_with_ops(
        &repo.root,
        Some("HEAD"),
        false,
        None,
        None,
        &[],
        &super::REAL_OPS,
    )
    .is_err());
}

#[test]
fn analysis_propagates_change_collection_failure_after_baseline_analysis() {
    let repo = Repo::new("changes-failure");
    repo.write("src/lib.rs", "pub fn stable() -> usize { 2 }\n");

    let ops = super::AnalysisOps {
        materialize_baseline: crate::git::materialize_worktree,
        analyze_baseline: baseline_then_break_git,
        sample_history: crate::git::sample_history,
        source_contexts: super::finding_source_contexts,
    };

    assert!(super::analyze_internal_with_ops(
        &repo.root,
        Some("HEAD"),
        false,
        None,
        None,
        &[],
        &ops,
    )
    .is_err());
}

#[test]
fn analysis_propagates_acceptance_corruption_between_baseline_and_finalization() {
    let repo = Repo::new("late-acceptance-corruption");
    repo.write("src/lib.rs", "pub fn stable() -> usize { 2 }\n");

    let ops = super::AnalysisOps {
        materialize_baseline: crate::git::materialize_worktree,
        analyze_baseline: baseline_then_corrupt_acceptance,
        sample_history: crate::git::sample_history,
        source_contexts: super::finding_source_contexts,
    };

    assert!(super::analyze_internal_with_ops(
        &repo.root,
        Some("HEAD"),
        false,
        None,
        None,
        &[],
        &ops,
    )
    .is_err());
}

#[test]
fn acceptance_propagates_profile_and_analysis_failures_before_lookup() {
    let repo = Repo::new("accept-propagation");

    assert!(super::accept_finding_with_profile(
        &repo.root,
        Some("HEAD"),
        Some("ferric-lens-invalid-target"),
        &[],
        "missing",
        "reason",
    )
    .is_err());

    let profile = crate::profile::ProfileContext::resolve(None, &[]).unwrap();
    let missing = repo.root.join("missing-root");
    assert!(super::accept_finding_with_profile(
        &missing,
        Some("HEAD"),
        Some(&profile.public.resolved_target),
        &[],
        "missing",
        "reason",
    )
    .is_err());
}

#[test]
fn finalize_findings_propagates_acceptance_load_failure() {
    let repo = Repo::new("finalize-acceptance-error");
    fs::create_dir_all(repo.root.join(".ferric-lens")).unwrap();
    fs::write(
        repo.root.join(".ferric-lens/acceptances.toml"),
        "not = [valid",
    )
    .unwrap();
    let mut findings = vec![finding("rule", "demo")];

    assert!(super::finalize_findings(&repo.root, "profile", &mut findings).is_err());
}
