use std::collections::BTreeSet;

use crate::{
    compare::{match_modules, Correspondence},
    git::ChangeSet,
    model::{DeltaStatus, EvidenceClass, Finding, ModuleMetrics, Priority},
};

use super::{
    current_snapshot_findings, evaluate_regressions, nearest_rank_p90, refactor_candidates,
    sort_findings,
};

fn module(index: usize, decisions: usize, dependencies: usize) -> ModuleMetrics {
    ModuleMetrics {
        crate_name: "demo".into(),
        module_path: format!("m{index}"),
        path: format!("src/m{index}.rs"),
        lines: 10,
        decision_sites: decisions,
        public_items: 0,
        clone_calls: 0,
        functions: Vec::new(),
        types: Vec::new(),
        explicit_imports: Vec::new(),
        local_dependency_modules: (0..dependencies).map(|n| format!("d{n}")).collect(),
        structure_digest: format!("digest-{index}"),
        parse_complete: true,
        gate_complete: true,
        limitation: None,
        history: None,
    }
}

fn baseline_population() -> Vec<ModuleMetrics> {
    let mut modules = Vec::new();
    modules.push(module(0, 12, 4));
    for index in 1..17 {
        modules.push(module(index, 10, 3));
    }
    for index in 17..20 {
        modules.push(module(index, 15, 5));
    }
    modules
}

#[test]
fn uses_nearest_rank_p90() {
    let values = (1..=20).collect::<Vec<_>>();
    assert_eq!(nearest_rank_p90(&values), 18);
}

#[test]
fn gates_only_when_both_growth_predicates_are_material() {
    let baseline = baseline_population();
    let mut head = baseline.clone();
    head[0].decision_sites = 18;
    head[0].local_dependency_modules = (0..7).map(|n| format!("d{n}")).collect();

    let mut changes = ChangeSet::default();
    changes.modified.insert(head[0].path.clone());
    let correspondence = match_modules(&baseline, &head, &changes);
    let evaluation = evaluate_regressions(&baseline, &head, &changes, &correspondence);

    assert_eq!(evaluation.applicable_subjects, 1);
    assert_eq!(evaluation.findings.len(), 1);
    assert!(evaluation.findings[0].gate);
}

#[test]
fn one_signal_growth_does_not_gate() {
    let baseline = baseline_population();
    let mut head = baseline.clone();
    head[0].decision_sites = 13;
    head[0].local_dependency_modules = (0..7).map(|n| format!("d{n}")).collect();

    let mut changes = ChangeSet::default();
    changes.modified.insert(head[0].path.clone());
    let correspondence = match_modules(&baseline, &head, &changes);
    let evaluation = evaluate_regressions(&baseline, &head, &changes, &correspondence);

    assert!(evaluation.findings.is_empty());
    assert!(evaluation.incomplete_reasons.is_empty());
}

#[test]
fn incomplete_baseline_population_is_not_silently_shrunk() {
    let mut baseline = baseline_population();
    baseline[5].gate_complete = false;
    let mut head = baseline.clone();
    head[0].decision_sites = 18;
    head[0].local_dependency_modules = (0..7).map(|n| format!("d{n}")).collect();

    let mut changes = ChangeSet::default();
    changes.modified.insert(head[0].path.clone());
    let correspondence = match_modules(&baseline, &head, &changes);
    let evaluation = evaluate_regressions(&baseline, &head, &changes, &correspondence);

    assert!(evaluation.findings.is_empty());
    assert_eq!(evaluation.incomplete_reasons.len(), 1);
}

#[test]
fn current_snapshot_reports_only_coupled_outliers_in_large_populations() {
    let mut modules = (0..20)
        .map(|index| module(index, 10, 3))
        .collect::<Vec<_>>();
    modules[0].decision_sites = 20;
    modules[0].local_dependency_modules = (0..8).map(|n| format!("d{n}")).collect();

    let findings = current_snapshot_findings(&modules);

    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].subject, "demo::m0");
    assert!(!findings[0].gate);
    assert_eq!(findings[0].delta, DeltaStatus::Current);

    assert!(!current_snapshot_findings(&modules[..19])
        .iter()
        .any(|finding| finding.rule == "structure.current_coupled_outlier"));
}

#[test]
fn current_snapshot_ignores_parse_incomplete_modules_in_population() {
    let mut modules = (0..20)
        .map(|index| module(index, 10, 3))
        .collect::<Vec<_>>();
    modules[0].parse_complete = false;

    assert!(current_snapshot_findings(&modules).is_empty());
}

#[test]
fn incomplete_head_evidence_is_explicitly_inconclusive() {
    let baseline = baseline_population();
    let mut head = baseline.clone();
    head[0].gate_complete = false;
    let mut changes = ChangeSet::default();
    changes.modified.insert(head[0].path.clone());
    let correspondence = match_modules(&baseline, &head, &changes);

    let evaluation = evaluate_regressions(&baseline, &head, &changes, &correspondence);

    assert_eq!(evaluation.applicable_subjects, 0);
    assert!(evaluation
        .incomplete_reasons
        .iter()
        .any(|reason| reason.contains("lacks complete evidence")));
}

#[test]
fn ambiguous_and_unmatched_changed_subjects_remain_incomplete() {
    let baseline = baseline_population();
    let head = baseline.clone();
    let mut changes = ChangeSet::default();
    changes.modified.insert(head[0].path.clone());

    let ambiguous = Correspondence {
        head_to_baseline: Default::default(),
        ambiguous_head: BTreeSet::from([0]),
    };
    let evaluation = evaluate_regressions(&baseline, &head, &changes, &ambiguous);
    assert!(evaluation
        .incomplete_reasons
        .iter()
        .any(|reason| reason.contains("ambiguous baseline correspondence")));

    let unmatched = Correspondence::default();
    let evaluation = evaluate_regressions(&baseline, &head, &changes, &unmatched);
    assert!(evaluation
        .incomplete_reasons
        .iter()
        .any(|reason| reason.contains("without reliable baseline correspondence")));
}

#[test]
fn genuinely_new_material_outlier_can_gate_against_existing_population() {
    let baseline = baseline_population();
    let mut head = baseline.clone();
    let mut added = module(20, 20, 8);
    added.path = "src/new.rs".into();
    added.module_path = "new".into();
    head.push(added.clone());

    let mut changes = ChangeSet::default();
    changes.added.insert(added.path.clone());
    let correspondence = Correspondence::default();

    let evaluation = evaluate_regressions(&baseline, &head, &changes, &correspondence);

    assert_eq!(evaluation.applicable_subjects, 1);
    assert_eq!(evaluation.findings.len(), 1);
    assert_eq!(evaluation.findings[0].delta, DeltaStatus::New);
    assert_eq!(evaluation.findings[0].identity, "demo::new");
}

#[test]
fn rename_destination_counts_as_changed_and_preserves_baseline_identity() {
    let baseline = baseline_population();
    let mut head = baseline.clone();
    head[0].path = "src/renamed.rs".into();
    head[0].module_path = "renamed".into();
    head[0].decision_sites = 18;
    head[0].local_dependency_modules = (0..7).map(|n| format!("d{n}")).collect();

    let mut changes = ChangeSet::default();
    changes
        .renames
        .insert("src/m0.rs".into(), "src/renamed.rs".into());
    let correspondence = Correspondence {
        head_to_baseline: [(0, 0)].into_iter().collect(),
        ambiguous_head: Default::default(),
    };

    let evaluation = evaluate_regressions(&baseline, &head, &changes, &correspondence);

    assert_eq!(evaluation.findings.len(), 1);
    assert_eq!(evaluation.findings[0].identity, "demo::m0");
}

#[test]
fn unchanged_and_small_crates_do_not_create_applicable_gate_subjects() {
    let baseline = baseline_population();
    let head = baseline.clone();
    let evaluation = evaluate_regressions(
        &baseline,
        &head,
        &ChangeSet::default(),
        &Correspondence::default(),
    );
    assert_eq!(evaluation.applicable_subjects, 0);
    assert!(evaluation.incomplete_reasons.is_empty());

    let evaluation = evaluate_regressions(
        &baseline[..19],
        &head[..19],
        &ChangeSet::default(),
        &Correspondence::default(),
    );
    assert_eq!(evaluation.applicable_subjects, 0);
}

#[test]
fn root_subject_and_sorting_are_deterministic() {
    let mut root = module(0, 20, 8);
    root.module_path.clear();
    let mut modules = (1..20)
        .map(|index| module(index, 10, 3))
        .collect::<Vec<_>>();
    modules.push(root);

    let findings = current_snapshot_findings(&modules);

    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].subject, "demo");
}

#[test]
fn finding_sort_orders_gates_first_then_rule_and_subject() {
    fn finding(gate: bool, rule: &str, subject: &str) -> Finding {
        Finding {
            fingerprint: String::new(),
            rule: rule.into(),
            subject: subject.into(),
            identity: subject.into(),
            configuration: String::new(),
            evidence_class: EvidenceClass::Candidate,
            priority: Priority::Observe,
            delta: DeltaStatus::Current,
            gate,
            accepted: false,
            acceptance_reason: None,
            summary: String::new(),
            direction: String::new(),
            evidence: Vec::new(),
        }
    }

    let mut findings = vec![
        finding(false, "b", "z"),
        finding(true, "b", "b"),
        finding(true, "a", "z"),
        finding(true, "a", "a"),
    ];

    sort_findings(&mut findings);

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
            (true, "a", "a"),
            (true, "a", "z"),
            (true, "b", "b"),
            (false, "b", "z")
        ]
    );
}

#[test]
fn runtime_clone_syntax_outlier_is_candidate_only() {
    let mut modules = (0..20).map(|index| module(index, 0, 0)).collect::<Vec<_>>();
    for module in &mut modules {
        module.clone_calls = 1;
    }
    modules[0].clone_calls = 7;

    let findings = current_snapshot_findings(&modules);
    let finding = findings
        .iter()
        .find(|finding| finding.rule == "runtime.clone_syntax_outlier")
        .expect("runtime candidate");

    assert_eq!(finding.subject, "demo::m0");
    assert_eq!(finding.evidence_class, EvidenceClass::Candidate);
    assert_eq!(finding.priority, Priority::Observe);
    assert!(!finding.gate);
    assert!(finding.summary.contains(".clone()"));
    assert!(finding.summary.contains("does not establish allocation"));
}

#[test]
fn build_reverse_dependency_outlier_is_candidate_only() {
    let mut modules = (0..20).map(|index| module(index, 0, 0)).collect::<Vec<_>>();
    for module in modules.iter_mut().skip(1) {
        module.local_dependency_modules = vec!["demo::m0".into()];
    }

    let findings = current_snapshot_findings(&modules);
    let finding = findings
        .iter()
        .find(|finding| finding.rule == "build.rebuild_exposure_candidate")
        .expect("build candidate");

    assert_eq!(finding.subject, "demo::m0");
    assert_eq!(finding.evidence_class, EvidenceClass::Candidate);
    assert_eq!(finding.priority, Priority::Observe);
    assert!(!finding.gate);
    assert!(finding.summary.contains("potential rebuild exposure"));
    assert!(finding.summary.contains("not measured compile cost"));
}

#[test]
fn small_population_emits_descriptive_candidates_without_gating() {
    let mut modules = (0..10)
        .map(|index| module(index, 10, 0))
        .collect::<Vec<_>>();
    modules[0].decision_sites = 40;
    modules[0].clone_calls = 8;
    for module in modules.iter_mut().skip(1) {
        module.local_dependency_modules = vec!["demo::m0".into()];
    }

    let findings = current_snapshot_findings(&modules);

    for rule in [
        "structure.small_population_decision_concentration",
        "runtime.small_population_clone_concentration",
        "build.small_population_rebuild_concentration",
    ] {
        let finding = findings
            .iter()
            .find(|finding| finding.rule == rule)
            .unwrap_or_else(|| panic!("missing descriptive finding {rule}"));
        assert_eq!(finding.evidence_class, EvidenceClass::Candidate);
        assert_eq!(finding.priority, Priority::Observe);
        assert!(!finding.gate);
        assert!(finding.summary.contains("small cohort"));
        assert!(finding.summary.contains("not a statistical outlier"));
        assert_eq!(finding.evidence[0].population, 10);
    }
}

#[test]
fn descriptive_small_population_candidates_require_four_subjects_and_a_unique_maximum() {
    let mut tiny = (0..3).map(|index| module(index, 10, 0)).collect::<Vec<_>>();
    tiny[0].decision_sites = 99;
    tiny[0].clone_calls = 99;
    for module in tiny.iter_mut().skip(1) {
        module.local_dependency_modules = vec!["demo::m0".into()];
    }
    assert!(current_snapshot_findings(&tiny).is_empty());

    let tied = (0..10)
        .map(|index| {
            let mut value = module(index, 10, 0);
            value.clone_calls = 2;
            value
        })
        .collect::<Vec<_>>();
    let findings = current_snapshot_findings(&tied);
    assert!(!findings.iter().any(|finding| {
        finding.rule == "structure.small_population_decision_concentration"
            || finding.rule == "runtime.small_population_clone_concentration"
    }));
}

#[test]
fn small_population_near_maxima_do_not_create_false_concentration_alerts() {
    let mut modules = (0..15)
        .map(|index| module(index, 20, 0))
        .collect::<Vec<_>>();

    modules[0].decision_sites = 128;
    modules[1].decision_sites = 119;
    modules[2].decision_sites = 95;

    modules[0].clone_calls = 22;
    modules[1].clone_calls = 21;
    modules[2].clone_calls = 19;

    let findings = current_snapshot_findings(&modules);

    assert!(!findings.iter().any(|finding| {
        finding.rule == "structure.small_population_decision_concentration"
            || finding.rule == "runtime.small_population_clone_concentration"
    }));
}

#[test]
fn small_population_clear_dependency_hub_remains_an_observation() {
    let mut modules = (0..16)
        .map(|index| module(index, 10, 0))
        .collect::<Vec<_>>();

    for index in 1..13 {
        modules[index].local_dependency_modules = vec!["demo::m0".into()];
    }
    for index in 13..16 {
        modules[index].local_dependency_modules = vec!["demo::m1".into()];
    }

    let finding = current_snapshot_findings(&modules)
        .into_iter()
        .find(|finding| finding.rule == "build.small_population_rebuild_concentration")
        .expect("clear dependency hub should remain visible");

    assert_eq!(finding.subject, "demo::m0");
    assert_eq!(finding.priority, Priority::Observe);
    assert_eq!(finding.evidence[0].value, 12);
    assert_eq!(finding.evidence[0].reference, 3);
}

#[test]
fn unique_max_helper_handles_empty_and_singleton_inputs_without_false_concentration() {
    assert_eq!(super::unique_max_above_median(&[]), None);
    assert_eq!(super::unique_max_above_median(&[7]), None);
    assert_eq!(super::unique_max_above_median(&[1, 2]), None);
}

fn advisory_finding(
    rule: &str,
    subject: &str,
    delta: DeltaStatus,
    evidence: Vec<crate::model::Evidence>,
) -> Finding {
    Finding {
        fingerprint: String::new(),
        rule: rule.into(),
        subject: subject.into(),
        identity: subject.into(),
        configuration: String::new(),
        evidence_class: EvidenceClass::Candidate,
        priority: Priority::Observe,
        delta,
        gate: false,
        accepted: false,
        acceptance_reason: None,
        summary: "supporting finding".into(),
        direction: "inspect".into(),
        evidence,
    }
}

fn evidence(metric: &str, value: usize) -> crate::model::Evidence {
    crate::model::Evidence {
        metric: metric.into(),
        value,
        reference: 10,
        population: 20,
        baseline: None,
        material_delta: None,
    }
}

#[test]
fn refactor_candidate_requires_two_independent_signal_kinds() {
    let complexity_only = vec![advisory_finding(
        "structure.decision_concentration",
        "demo::engine",
        DeltaStatus::Current,
        vec![
            evidence("decision_sites", 20),
            evidence("decision_sites", 21),
        ],
    )];

    assert!(refactor_candidates(&complexity_only).is_empty());

    let corroborated = vec![
        advisory_finding(
            "structure.decision_concentration",
            "demo::engine",
            DeltaStatus::Current,
            vec![evidence("decision_sites", 20)],
        ),
        advisory_finding(
            "build.rebuild_exposure_candidate",
            "demo::engine",
            DeltaStatus::Current,
            vec![evidence("reverse_repository_dependents", 15)],
        ),
    ];

    let candidates = refactor_candidates(&corroborated);

    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].rule, "refactor.multi_signal_candidate");
    assert_eq!(candidates[0].subject, "demo::engine");
    assert_eq!(candidates[0].priority, Priority::Investigate);
    assert_eq!(candidates[0].evidence_class, EvidenceClass::Strong);
    assert!(!candidates[0].gate);
    assert!(candidates[0].summary.contains("2 independent signals"));
    assert!(candidates[0].summary.contains("decision complexity"));
    assert!(candidates[0].summary.contains("dependency surface"));
}

#[test]
fn structural_coupled_outlier_is_already_multi_signal_refactor_evidence() {
    let supporting = vec![advisory_finding(
        "structure.current_coupled_outlier",
        "demo::engine",
        DeltaStatus::Current,
        vec![
            evidence("decision_sites", 20),
            evidence("local_dependency_modules", 8),
        ],
    )];

    let candidates = refactor_candidates(&supporting);

    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].evidence.len(), 2);
    assert!(candidates[0]
        .direction
        .contains("splitting responsibilities"));
    assert!(candidates[0]
        .direction
        .contains("narrowing dependency surface"));
}

#[test]
fn refactor_candidate_combines_support_deterministically_and_keeps_change_relevance() {
    let supporting = vec![
        advisory_finding(
            "runtime.clone_syntax_outlier",
            "demo::engine",
            DeltaStatus::Current,
            vec![evidence("clone_call_syntax_sites", 12)],
        ),
        advisory_finding(
            "build.rebuild_exposure_candidate",
            "demo::engine",
            DeltaStatus::Current,
            vec![evidence("reverse_repository_dependents", 15)],
        ),
        Finding {
            delta: DeltaStatus::Worsened,
            gate: true,
            evidence_class: EvidenceClass::Strong,
            priority: Priority::ActFirst,
            ..advisory_finding(
                "structure.coupled_complexity_growth",
                "demo::engine",
                DeltaStatus::Worsened,
                vec![
                    evidence("local_dependency_modules", 8),
                    evidence("decision_sites", 20),
                ],
            )
        },
    ];

    let first = refactor_candidates(&supporting);
    let mut reversed = supporting;
    reversed.reverse();
    let second = refactor_candidates(&reversed);

    assert_eq!(first, second);
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].delta, DeltaStatus::Worsened);
    assert_eq!(
        first[0]
            .evidence
            .iter()
            .map(|item| item.metric.as_str())
            .collect::<Vec<_>>(),
        vec![
            "clone_call_syntax_sites",
            "decision_sites",
            "local_dependency_modules",
            "reverse_repository_dependents",
        ]
    );
    assert!(first[0].summary.contains("3 independent signals"));
}

#[test]
fn refactor_synthesis_does_not_use_other_refactor_findings_as_support() {
    let supporting = vec![
        advisory_finding(
            "refactor.previous_candidate",
            "demo::engine",
            DeltaStatus::Current,
            vec![
                evidence("decision_sites", 20),
                evidence("local_dependency_modules", 8),
            ],
        ),
        advisory_finding(
            "runtime.clone_syntax_outlier",
            "demo::engine",
            DeltaStatus::Current,
            vec![evidence("clone_call_syntax_sites", 12)],
        ),
    ];

    assert!(refactor_candidates(&supporting).is_empty());
}

#[test]
fn refactor_candidate_preserves_only_unambiguous_prior_identity() {
    let mut moved = advisory_finding(
        "structure.current_coupled_outlier",
        "demo::renamed",
        DeltaStatus::Current,
        vec![
            evidence("decision_sites", 20),
            evidence("local_dependency_modules", 8),
        ],
    );
    moved.identity = "demo::original".into();

    let candidate = refactor_candidates(&[moved.clone()]).remove(0);
    assert_eq!(candidate.identity, "demo::original");

    let mut conflicting = advisory_finding(
        "runtime.clone_syntax_outlier",
        "demo::renamed",
        DeltaStatus::Current,
        vec![evidence("clone_call_syntax_sites", 12)],
    );
    conflicting.identity = "demo::other".into();

    let candidate = refactor_candidates(&[moved, conflicting]).remove(0);
    assert_eq!(candidate.identity, "demo::renamed");
}

#[test]
fn refactor_candidate_ignores_unrecognized_metrics() {
    let findings = vec![
        advisory_finding(
            "structure.decision_concentration",
            "demo::engine",
            DeltaStatus::Current,
            vec![evidence("decision_sites", 20)],
        ),
        advisory_finding(
            "other.public_surface",
            "demo::engine",
            DeltaStatus::Current,
            vec![evidence("public_items", 12)],
        ),
    ];

    assert!(refactor_candidates(&findings).is_empty());

    let mixed = vec![
        advisory_finding(
            "structure.decision_concentration",
            "demo::engine",
            DeltaStatus::Current,
            vec![evidence("decision_sites", 20), evidence("public_items", 12)],
        ),
        advisory_finding(
            "build.rebuild_exposure_candidate",
            "demo::engine",
            DeltaStatus::Current,
            vec![evidence("reverse_repository_dependents", 15)],
        ),
    ];
    let candidate = refactor_candidates(&mixed).remove(0);
    assert!(candidate
        .evidence
        .iter()
        .all(|item| item.metric != "public_items"));
}

#[test]
fn refactor_delta_preserves_new_unchanged_and_unknown_states() {
    let new = advisory_finding(
        "structure.test",
        "demo::engine",
        DeltaStatus::New,
        Vec::new(),
    );
    let unchanged = advisory_finding(
        "structure.test",
        "demo::engine",
        DeltaStatus::Unchanged,
        Vec::new(),
    );
    let unknown = advisory_finding(
        "structure.test",
        "demo::engine",
        DeltaStatus::Unknown,
        Vec::new(),
    );

    assert_eq!(super::refactor_delta(&[&new]), DeltaStatus::New);
    assert_eq!(super::refactor_delta(&[&unchanged]), DeltaStatus::Unchanged);
    assert_eq!(super::refactor_delta(&[&unknown]), DeltaStatus::Unknown);
}

#[test]
fn refactor_directions_cover_each_supported_signal_pair() {
    assert!(super::refactor_direction(BTreeSet::from([
        "decision_complexity",
        "copying_runtime_risk",
    ]))
    .contains("complex orchestration"));

    assert!(super::refactor_direction(BTreeSet::from([
        "dependency_surface",
        "copying_runtime_risk",
    ]))
    .contains("stable dependency boundary"));

    assert!(super::refactor_direction(BTreeSet::new())
        .contains("investigate the corroborating evidence"));
}
