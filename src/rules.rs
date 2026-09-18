use std::collections::{BTreeMap, BTreeSet};

use crate::{
    compare::Correspondence,
    git::ChangeSet,
    model::{
        DeltaStatus, Evidence, EvidenceClass, Finding, ModuleMetrics, Priority,
    },
};

const MIN_POPULATION: usize = 20;

#[derive(Debug, Default)]
pub struct GateEvaluation {
    pub findings: Vec<Finding>,
    pub applicable_subjects: usize,
    pub incomplete_reasons: Vec<String>,
}

pub fn current_snapshot_findings(modules: &[ModuleMetrics]) -> Vec<Finding> {
    let mut by_crate = BTreeMap::<&str, Vec<&ModuleMetrics>>::new();
    for module in modules.iter().filter(|module| module.parse_complete) {
        by_crate.entry(&module.crate_name).or_default().push(module);
    }

    let mut findings = Vec::new();
    for (crate_name, population) in by_crate {
        if population.len() < MIN_POPULATION {
            continue;
        }

        let decision_values = population
            .iter()
            .map(|module| module.decision_sites)
            .collect::<Vec<_>>();
        let dependency_values = population
            .iter()
            .map(|module| module.local_dependency_modules.len())
            .collect::<Vec<_>>();
        let decision_p90 = nearest_rank_p90(&decision_values);
        let dependency_p90 = nearest_rank_p90(&dependency_values);

        for module in population {
            let dependencies = module.local_dependency_modules.len();
            if module.decision_sites > decision_p90 && dependencies > dependency_p90 {
                findings.push(Finding {
                    rule: "structure.current_coupled_outlier".into(),
                    subject: subject(crate_name, &module.module_path),
                    evidence_class: EvidenceClass::Strong,
                    priority: Priority::Investigate,
                    delta: DeltaStatus::Current,
                    gate: false,
                    summary: "module is simultaneously an outlier for decision sites and local dependency breadth".into(),
                    direction: "investigate whether responsibility and dependency surface can be reduced without changing behavior".into(),
                    evidence: vec![
                        Evidence {
                            metric: "decision_sites".into(),
                            value: module.decision_sites,
                            reference: decision_p90,
                            population: decision_values.len(),
                            baseline: None,
                            material_delta: None,
                        },
                        Evidence {
                            metric: "local_dependency_modules".into(),
                            value: dependencies,
                            reference: dependency_p90,
                            population: dependency_values.len(),
                            baseline: None,
                            material_delta: None,
                        },
                    ],
                });
            }
        }
    }

    sort_findings(&mut findings);
    findings
}

pub fn evaluate_regressions(
    baseline: &[ModuleMetrics],
    head: &[ModuleMetrics],
    changes: &ChangeSet,
    correspondence: &Correspondence,
) -> GateEvaluation {
    let mut evaluation = GateEvaluation::default();
    let mut incomplete = BTreeSet::new();

    let mut baseline_by_crate = BTreeMap::<&str, Vec<&ModuleMetrics>>::new();
    for module in baseline {
        baseline_by_crate
            .entry(&module.crate_name)
            .or_default()
            .push(module);
    }

    for (crate_name, population) in baseline_by_crate {
        if population.len() < MIN_POPULATION {
            continue;
        }

        let changed_head = head
            .iter()
            .enumerate()
            .filter(|(_, module)| {
                module.crate_name == crate_name && path_changed(&module.path, changes)
            })
            .collect::<Vec<_>>();
        if changed_head.is_empty() {
            continue;
        }

        if population.iter().any(|module| !module.gate_complete) {
            incomplete.insert(format!(
                "crate {crate_name} has an incomplete baseline population for structure.coupled_complexity_growth"
            ));
            continue;
        }

        let decision_values = population
            .iter()
            .map(|module| module.decision_sites)
            .collect::<Vec<_>>();
        let dependency_values = population
            .iter()
            .map(|module| module.local_dependency_modules.len())
            .collect::<Vec<_>>();
        let decision_p90 = nearest_rank_p90(&decision_values);
        let dependency_p90 = nearest_rank_p90(&dependency_values);

        for (head_index, module) in changed_head {
            if !module.gate_complete {
                incomplete.insert(format!(
                    "{} lacks complete evidence for structure.coupled_complexity_growth",
                    subject(crate_name, &module.module_path)
                ));
                continue;
            }

            let (baseline_decisions, baseline_dependencies, delta) =
                if let Some(&baseline_index) = correspondence.head_to_baseline.get(&head_index) {
                    let previous = &baseline[baseline_index];
                    (
                        previous.decision_sites,
                        previous.local_dependency_modules.len(),
                        DeltaStatus::Worsened,
                    )
                } else if correspondence.ambiguous_head.contains(&head_index) {
                    incomplete.insert(format!(
                        "{} has ambiguous baseline correspondence",
                        subject(crate_name, &module.module_path)
                    ));
                    continue;
                } else if changes.added.contains(&module.path) {
                    (0, 0, DeltaStatus::New)
                } else {
                    incomplete.insert(format!(
                        "{} changed without reliable baseline correspondence",
                        subject(crate_name, &module.module_path)
                    ));
                    continue;
                };

            evaluation.applicable_subjects += 1;

            let dependencies = module.local_dependency_modules.len();
            let decision_growth = module.decision_sites.saturating_sub(baseline_decisions);
            let dependency_growth = dependencies.saturating_sub(baseline_dependencies);
            let decision_material = baseline_decisions.div_ceil(4).max(3);
            let dependency_material = baseline_dependencies.div_ceil(4).max(2);

            let regressed = module.decision_sites > decision_p90
                && dependencies > dependency_p90
                && decision_growth >= decision_material
                && dependency_growth >= dependency_material;

            if regressed {
                evaluation.findings.push(Finding {
                    rule: "structure.coupled_complexity_growth".into(),
                    subject: subject(crate_name, &module.module_path),
                    evidence_class: EvidenceClass::Strong,
                    priority: Priority::ActFirst,
                    delta,
                    gate: true,
                    summary: "module materially increased both decision-site count and local dependency breadth beyond the frozen baseline p90".into(),
                    direction: "inspect whether the change combines responsibilities or broadens dependency surface unnecessarily before merging".into(),
                    evidence: vec![
                        Evidence {
                            metric: "decision_sites".into(),
                            value: module.decision_sites,
                            reference: decision_p90,
                            population: decision_values.len(),
                            baseline: Some(baseline_decisions),
                            material_delta: Some(decision_material),
                        },
                        Evidence {
                            metric: "local_dependency_modules".into(),
                            value: dependencies,
                            reference: dependency_p90,
                            population: dependency_values.len(),
                            baseline: Some(baseline_dependencies),
                            material_delta: Some(dependency_material),
                        },
                    ],
                });
            }
        }
    }

    evaluation.incomplete_reasons = incomplete.into_iter().collect();
    sort_findings(&mut evaluation.findings);
    evaluation
}

fn path_changed(path: &str, changes: &ChangeSet) -> bool {
    changes.added.contains(path)
        || changes.modified.contains(path)
        || changes.renames.values().any(|new| new == path)
}

fn subject(crate_name: &str, module_path: &str) -> String {
    if module_path.is_empty() {
        crate_name.to_owned()
    } else {
        format!("{crate_name}::{module_path}")
    }
}

fn sort_findings(findings: &mut [Finding]) {
    findings.sort_by(|a, b| {
        b.gate
            .cmp(&a.gate)
            .then_with(|| (&a.rule, &a.subject).cmp(&(&b.rule, &b.subject)))
    });
}

fn nearest_rank_p90(values: &[usize]) -> usize {
    debug_assert!(!values.is_empty());
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let rank = (9 * sorted.len()).div_ceil(10);
    sorted[rank.saturating_sub(1)]
}

#[cfg(test)]
mod tests {
    use crate::{
        compare::match_modules,
        git::ChangeSet,
        model::ModuleMetrics,
    };

    use super::{evaluate_regressions, nearest_rank_p90};

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
}
