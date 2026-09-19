use std::collections::{BTreeMap, BTreeSet};

use crate::{
    compare::Correspondence,
    git::ChangeSet,
    model::{DeltaStatus, Evidence, EvidenceClass, Finding, ModuleMetrics, Priority},
};

const MIN_POPULATION: usize = 20;
const MIN_DESCRIPTIVE_POPULATION: usize = 4;
const MIN_DESCRIPTIVE_POPULATION: usize = 4;

#[derive(Debug, Default)]
pub struct GateEvaluation {
    pub findings: Vec<Finding>,
    pub applicable_subjects: usize,
    pub incomplete_reasons: Vec<String>,
}

pub fn current_snapshot_findings(modules: &[ModuleMetrics]) -> Vec<Finding> {
    let mut findings = structural_coupled_outliers(modules);
    findings.extend(small_population_decision_concentrations(modules));
    findings.extend(runtime_clone_syntax_outliers(modules));
    findings.extend(small_population_clone_concentrations(modules));
    findings.extend(build_rebuild_exposure_candidates(modules));
    findings.extend(small_population_rebuild_concentrations(modules));
    sort_findings(&mut findings);
    findings
}

fn structural_coupled_outliers(modules: &[ModuleMetrics]) -> Vec<Finding> {
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
                let subject = subject(crate_name, &module.module_path);
                findings.push(Finding {
                    fingerprint: String::new(),
                    rule: "structure.current_coupled_outlier".into(),
                    subject: subject.clone(),
                    identity: subject,
                    configuration: String::new(),
                    evidence_class: EvidenceClass::Strong,
                    priority: Priority::Investigate,
                    delta: DeltaStatus::Current,
                    gate: false,
                    accepted: false,
                    acceptance_reason: None,
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

    findings
}

fn small_population_decision_concentrations(modules: &[ModuleMetrics]) -> Vec<Finding> {
    let mut by_crate = BTreeMap::<&str, Vec<&ModuleMetrics>>::new();
    for module in modules.iter().filter(|module| module.parse_complete) {
        by_crate.entry(&module.crate_name).or_default().push(module);
    }

    let mut findings = Vec::new();
    for (crate_name, population) in by_crate {
        if population.len() < MIN_DESCRIPTIVE_POPULATION || population.len() >= MIN_POPULATION {
            continue;
        }
        let values = population
            .iter()
            .map(|module| module.decision_sites)
            .collect::<Vec<_>>();
        let Some((index, value, reference)) = unique_max_above_median(&values) else {
            continue;
        };
        let module = population[index];
        let module_subject = subject(crate_name, &module.module_path);
        findings.push(Finding {
            fingerprint: String::new(),
            rule: "structure.small_population_decision_concentration".into(),
            subject: module_subject.clone(),
            identity: module_subject,
            configuration: String::new(),
            evidence_class: EvidenceClass::Candidate,
            priority: Priority::Observe,
            delta: DeltaStatus::Current,
            gate: false,
            accepted: false,
            acceptance_reason: None,
            summary: "module has the highest observed decision-site count in a small cohort; this is descriptive concentration, not a statistical outlier".into(),
            direction: "inspect whether the module concentrates multiple responsibilities; the cohort is too small for percentile-based classification".into(),
            evidence: vec![Evidence {
                metric: "decision_sites".into(),
                value,
                reference,
                population: values.len(),
                baseline: None,
                material_delta: None,
            }],
        });
    }
    findings
}

fn small_population_clone_concentrations(modules: &[ModuleMetrics]) -> Vec<Finding> {
    let mut by_crate = BTreeMap::<&str, Vec<&ModuleMetrics>>::new();
    for module in modules.iter().filter(|module| module.parse_complete) {
        by_crate.entry(&module.crate_name).or_default().push(module);
    }

    let mut findings = Vec::new();
    for (crate_name, population) in by_crate {
        if population.len() < MIN_DESCRIPTIVE_POPULATION || population.len() >= MIN_POPULATION {
            continue;
        }
        let values = population
            .iter()
            .map(|module| module.clone_calls)
            .collect::<Vec<_>>();
        let Some((index, value, reference)) = unique_max_above_median(&values) else {
            continue;
        };
        if value == 0 {
            continue;
        }
        let module = population[index];
        let module_subject = subject(crate_name, &module.module_path);
        findings.push(Finding {
            fingerprint: String::new(),
            rule: "runtime.small_population_clone_concentration".into(),
            subject: module_subject.clone(),
            identity: module_subject,
            configuration: String::new(),
            evidence_class: EvidenceClass::Candidate,
            priority: Priority::Observe,
            delta: DeltaStatus::Current,
            gate: false,
            accepted: false,
            acceptance_reason: None,
            summary: "module has the highest observed .clone() syntax count in a small cohort; this is descriptive concentration, not a statistical outlier and does not establish allocation or runtime cost".into(),
            direction: "inspect receiver types and execution frequency, then measure before changing copying or allocation behavior".into(),
            evidence: vec![Evidence {
                metric: "clone_call_syntax_sites".into(),
                value,
                reference,
                population: values.len(),
                baseline: None,
                material_delta: None,
            }],
        });
    }
    findings
}

fn small_population_rebuild_concentrations(modules: &[ModuleMetrics]) -> Vec<Finding> {
    let eligible = modules
        .iter()
        .filter(|module| module.parse_complete)
        .collect::<Vec<_>>();
    if eligible.len() < MIN_DESCRIPTIVE_POPULATION || eligible.len() >= MIN_POPULATION {
        return Vec::new();
    }

    let mut reverse_dependents = eligible
        .iter()
        .map(|module| (subject(&module.crate_name, &module.module_path), 0usize))
        .collect::<BTreeMap<_, _>>();
    for module in &eligible {
        for dependency in &module.local_dependency_modules {
            if let Some(count) = reverse_dependents.get_mut(dependency) {
                *count += 1;
            }
        }
    }

    let values = eligible
        .iter()
        .map(|module| reverse_dependents[&subject(&module.crate_name, &module.module_path)])
        .collect::<Vec<_>>();
    let Some((index, value, reference)) = unique_max_above_median(&values) else {
        return Vec::new();
    };
    if value == 0 {
        return Vec::new();
    }
    let module = eligible[index];
    let module_subject = subject(&module.crate_name, &module.module_path);
    vec![Finding {
        fingerprint: String::new(),
        rule: "build.small_population_rebuild_concentration".into(),
        subject: module_subject.clone(),
        identity: module_subject,
        configuration: String::new(),
        evidence_class: EvidenceClass::Candidate,
        priority: Priority::Observe,
        delta: DeltaStatus::Current,
        gate: false,
        accepted: false,
        acceptance_reason: None,
        summary: "module has the highest observed reverse repository dependency reach in a small cohort; this is descriptive concentration, not a statistical outlier or measured compile cost".into(),
        direction: "inspect whether this dependency boundary can remain stable or narrower, then measure incremental build impact before optimizing it".into(),
        evidence: vec![Evidence {
            metric: "reverse_repository_dependents".into(),
            value,
            reference,
            population: values.len(),
            baseline: None,
            material_delta: None,
        }],
    }]
}

fn runtime_clone_syntax_outliers(modules: &[ModuleMetrics]) -> Vec<Finding> {
    let mut by_crate = BTreeMap::<&str, Vec<&ModuleMetrics>>::new();
    for module in modules.iter().filter(|module| module.parse_complete) {
        by_crate.entry(&module.crate_name).or_default().push(module);
    }

    let mut findings = Vec::new();
    for (crate_name, population) in by_crate {
        if population.len() < MIN_POPULATION {
            continue;
        }
        let values = population
            .iter()
            .map(|module| module.clone_calls)
            .collect::<Vec<_>>();
        let p90 = nearest_rank_p90(&values);

        for module in population {
            if module.clone_calls <= p90 {
                continue;
            }
            let subject = subject(crate_name, &module.module_path);
            findings.push(Finding {
                fingerprint: String::new(),
                rule: "runtime.clone_syntax_outlier".into(),
                subject: subject.clone(),
                identity: subject,
                configuration: String::new(),
                evidence_class: EvidenceClass::Candidate,
                priority: Priority::Observe,
                delta: DeltaStatus::Current,
                gate: false,
                accepted: false,
                acceptance_reason: None,
                summary: "module is a repository-relative outlier for observed .clone() method-call syntax; this proves source syntax only and does not establish allocation, unnecessary copying, execution frequency, or a runtime bottleneck".into(),
                direction: "inspect receiver types and execution frequency, then measure before changing copying or allocation behavior".into(),
                evidence: vec![Evidence {
                    metric: "clone_call_syntax_sites".into(),
                    value: module.clone_calls,
                    reference: p90,
                    population: values.len(),
                    baseline: None,
                    material_delta: None,
                }],
            });
        }
    }
    findings
}

fn build_rebuild_exposure_candidates(modules: &[ModuleMetrics]) -> Vec<Finding> {
    let eligible = modules
        .iter()
        .filter(|module| module.parse_complete)
        .collect::<Vec<_>>();
    if eligible.len() < MIN_POPULATION {
        return Vec::new();
    }

    let mut reverse_dependents = eligible
        .iter()
        .map(|module| (subject(&module.crate_name, &module.module_path), 0usize))
        .collect::<BTreeMap<_, _>>();

    for module in &eligible {
        for dependency in &module.local_dependency_modules {
            if let Some(count) = reverse_dependents.get_mut(dependency) {
                *count += 1;
            }
        }
    }

    let values = reverse_dependents.values().copied().collect::<Vec<_>>();
    let p90 = nearest_rank_p90(&values);
    let mut findings = Vec::new();

    for module in eligible {
        let module_subject = subject(&module.crate_name, &module.module_path);
        let dependents = reverse_dependents[&module_subject];
        if dependents <= p90 {
            continue;
        }
        findings.push(Finding {
            fingerprint: String::new(),
            rule: "build.rebuild_exposure_candidate".into(),
            subject: module_subject.clone(),
            identity: module_subject,
            configuration: String::new(),
            evidence_class: EvidenceClass::Candidate,
            priority: Priority::Observe,
            delta: DeltaStatus::Current,
            gate: false,
            accepted: false,
            acceptance_reason: None,
            summary: "module has unusually broad reverse repository dependency reach, indicating potential rebuild exposure; this is a structural proxy, not measured compile cost".into(),
            direction: "inspect whether this dependency boundary can remain stable or narrower, and measure incremental build impact before optimizing it".into(),
            evidence: vec![Evidence {
                metric: "reverse_repository_dependents".into(),
                value: dependents,
                reference: p90,
                population: values.len(),
                baseline: None,
                material_delta: None,
            }],
        });
    }

    findings
}

fn small_population_candidates(modules: &[ModuleMetrics]) -> Vec<Finding> {
    let mut findings = Vec::new();
    let mut by_crate = BTreeMap::<&str, Vec<&ModuleMetrics>>::new();
    for module in modules.iter().filter(|module| module.parse_complete) {
        by_crate.entry(&module.crate_name).or_default().push(module);
    }

    for (crate_name, population) in by_crate {
        if let Some((index, value, reference)) = unique_small_population_max(
            &population
                .iter()
                .map(|module| module.decision_sites)
                .collect::<Vec<_>>(),
        ) {
            let module = population[index];
            findings.push(descriptive_candidate(
                "structure.small_population_decision_concentration",
                subject(crate_name, &module.module_path),
                "decision_sites",
                value,
                reference,
                population.len(),
                "small cohort has one module with the highest observed decision-site count; this is descriptive concentration, not a statistical outlier",
                "inspect whether the concentrated decision structure reflects mixed responsibilities, but do not treat the small cohort as statistical evidence",
            ));
        }

        if let Some((index, value, reference)) = unique_small_population_max(
            &population
                .iter()
                .map(|module| module.clone_calls)
                .collect::<Vec<_>>(),
        ) {
            let module = population[index];
            findings.push(descriptive_candidate(
                "runtime.small_population_clone_concentration",
                subject(crate_name, &module.module_path),
                "clone_call_syntax_sites",
                value,
                reference,
                population.len(),
                "small cohort has one module with the highest observed .clone() syntax count; this is descriptive concentration, not a statistical outlier or measured runtime cost",
                "inspect receiver types and execution frequency before considering copying or allocation changes",
            ));
        }
    }

    let eligible = modules
        .iter()
        .filter(|module| module.gate_complete)
        .collect::<Vec<_>>();
    if (MIN_DESCRIPTIVE_POPULATION..MIN_POPULATION).contains(&eligible.len()) {
        let mut reverse_dependents = eligible
            .iter()
            .map(|module| (subject(&module.crate_name, &module.module_path), 0usize))
            .collect::<BTreeMap<_, _>>();
        for module in &eligible {
            for dependency in &module.local_dependency_modules {
                if let Some(count) = reverse_dependents.get_mut(dependency) {
                    *count += 1;
                }
            }
        }

        let values = eligible
            .iter()
            .map(|module| reverse_dependents[&subject(&module.crate_name, &module.module_path)])
            .collect::<Vec<_>>();
        if let Some((index, value, reference)) = unique_small_population_max(&values) {
            let module = eligible[index];
            findings.push(descriptive_candidate(
                "build.small_population_rebuild_concentration",
                subject(&module.crate_name, &module.module_path),
                "reverse_repository_dependents",
                value,
                reference,
                eligible.len(),
                "small cohort has one module with the highest reverse repository dependency reach; this is descriptive concentration, not a statistical outlier or measured compile cost",
                "inspect whether this dependency boundary can remain stable or narrower, and measure incremental build impact before optimizing it",
            ));
        }
    }

    findings
}

fn unique_small_population_max(values: &[usize]) -> Option<(usize, usize, usize)> {
    if !(MIN_DESCRIPTIVE_POPULATION..MIN_POPULATION).contains(&values.len()) {
        return None;
    }

    let maximum = values.iter().copied().max()?;
    if maximum == 0 || values.iter().filter(|value| **value == maximum).count() != 1 {
        return None;
    }
    let index = values.iter().position(|value| *value == maximum)?;
    let reference = values
        .iter()
        .enumerate()
        .filter(|(candidate, _)| *candidate != index)
        .map(|(_, value)| *value)
        .max()
        .unwrap_or(0);
    Some((index, maximum, reference))
}

#[allow(clippy::too_many_arguments)]
fn descriptive_candidate(
    rule: &str,
    subject: String,
    metric: &str,
    value: usize,
    reference: usize,
    population: usize,
    summary: &str,
    direction: &str,
) -> Finding {
    Finding {
        fingerprint: String::new(),
        rule: rule.into(),
        subject: subject.clone(),
        identity: subject,
        configuration: String::new(),
        evidence_class: EvidenceClass::Candidate,
        priority: Priority::Observe,
        delta: DeltaStatus::Current,
        gate: false,
        accepted: false,
        acceptance_reason: None,
        summary: summary.into(),
        direction: direction.into(),
        evidence: vec![Evidence {
            metric: metric.into(),
            value,
            reference,
            population,
            baseline: None,
            material_delta: None,
        }],
    }
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

            let (baseline_decisions, baseline_dependencies, delta, identity) =
                if let Some(&baseline_index) = correspondence.head_to_baseline.get(&head_index) {
                    let previous = &baseline[baseline_index];
                    (
                        previous.decision_sites,
                        previous.local_dependency_modules.len(),
                        DeltaStatus::Worsened,
                        subject(&previous.crate_name, &previous.module_path),
                    )
                } else if correspondence.ambiguous_head.contains(&head_index) {
                    incomplete.insert(format!(
                        "{} has ambiguous baseline correspondence",
                        subject(crate_name, &module.module_path)
                    ));
                    continue;
                } else if changes.added.contains(&module.path) {
                    (
                        0,
                        0,
                        DeltaStatus::New,
                        subject(crate_name, &module.module_path),
                    )
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
                    fingerprint: String::new(),
                    rule: "structure.coupled_complexity_growth".into(),
                    subject: subject(crate_name, &module.module_path),
                    identity,
                    configuration: String::new(),
                    evidence_class: EvidenceClass::Strong,
                    priority: Priority::ActFirst,
                    delta,
                    gate: true,
                    accepted: false,
                    acceptance_reason: None,
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

fn unique_max_above_median(values: &[usize]) -> Option<(usize, usize, usize)> {
    if values.is_empty() {
        return None;
    }
    let max = *values.iter().max()?;
    if values.iter().filter(|value| **value == max).count() != 1 {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let median = sorted[sorted.len() / 2];
    if max <= median {
        return None;
    }
    let index = values.iter().position(|value| *value == max)?;
    Some((index, max, median))
}

fn nearest_rank_p90(values: &[usize]) -> usize {
    debug_assert!(!values.is_empty());
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let rank = (9 * sorted.len()).div_ceil(10);
    sorted[rank.saturating_sub(1)]
}

#[cfg(test)]
#[path = "rules_tests.rs"]
mod tests;
