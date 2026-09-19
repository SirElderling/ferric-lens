    use std::collections::BTreeSet;

    use crate::{
        compare::{match_modules, Correspondence},
        git::ChangeSet,
        model::{DeltaStatus, ModuleMetrics},
    };

    use super::{current_snapshot_findings, evaluate_regressions, nearest_rank_p90};

    fn module(index: usize, decisions: usize, dependencies: usize) -> ModuleMetrics {
        ModuleMetrics {
            crate_name: "demo".into(),
            module_path: format!("m{index}"),
            path: format!("src/m{index}.rs"),
            lines: 10,
            decision_sites: decisions,
            public_items: 0,
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

        assert!(current_snapshot_findings(&modules[..19]).is_empty());
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
