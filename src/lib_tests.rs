use std::collections::{BTreeMap, BTreeSet};

use crate::{
    compare::Correspondence,
    git::ChangeSet,
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
    super::rebind_advisory_identities(
        &mut no_match,
        &head,
        &baseline,
        &Correspondence::default(),
    );
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
