use std::collections::{BTreeMap, BTreeSet};

use crate::{
    compare::Correspondence,
    git::ChangeSet,
    model::{DeltaStatus, Evidence, EvidenceClass, Finding, ModuleMetrics, Priority},
};

const MIN_POPULATION: usize = 20;
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

pub fn refactor_candidates(findings: &[Finding]) -> Vec<Finding> {
    let mut by_subject = BTreeMap::<&str, Vec<&Finding>>::new();
    for finding in findings
        .iter()
        .filter(|finding| !finding.rule.starts_with("refactor."))
    {
        if finding.evidence.iter().any(|item| refactor_signal(&item.metric).is_some()) {
            by_subject.entry(&finding.subject).or_default().push(finding);
        }
    }

    let mut candidates = Vec::new();
    for (subject, supporting) in by_subject {
        let mut signals = BTreeMap::<&str, &str>::new();
        let mut evidence = Vec::new();

        for finding in &supporting {
            for item in &finding.evidence {
                let Some((signal, label)) = refactor_signal(&item.metric) else {
                    continue;
                };
                signals.insert(signal, label);
                evidence.push(item.clone());
            }
        }

        if signals.len() < 2 {
            continue;
        }

        evidence.sort_by(|a, b| {
            (
                &a.metric,
                a.value,
                a.reference,
                a.population,
                a.baseline,
                a.material_delta,
            )
                .cmp(&(
                    &b.metric,
                    b.value,
                    b.reference,
                    b.population,
                    b.baseline,
                    b.material_delta,
                ))
        });
        evidence.dedup();

        let labels = signals.values().copied().collect::<Vec<_>>();
        let identity = refactor_identity(subject, &supporting);
        let delta = refactor_delta(&supporting);
        let direction = refactor_direction(signals.keys().copied().collect());

        candidates.push(Finding {
            fingerprint: String::new(),
            rule: "refactor.multi_signal_candidate".into(),
            subject: subject.to_owned(),
            identity,
            configuration: String::new(),
            evidence_class: EvidenceClass::Strong,
            priority: Priority::Investigate,
            delta,
            gate: false,
            accepted: false,
            acceptance_reason: None,
            summary: format!(
                "module has corroborating refactor evidence across {} independent signals: {}",
                labels.len(),
                labels.join(", ")
            ),
            direction,
            evidence,
        });
    }

    sort_findings(&mut candidates);
    candidates
}

fn refactor_signal(metric: &str) -> Option<(&'static str, &'static str)> {
    match metric {
        "decision_sites" => Some(("decision_complexity", "decision complexity")),
        "local_dependency_modules" | "reverse_repository_dependents" => {
            Some(("dependency_surface", "dependency surface"))
        }
        "clone_call_syntax_sites" => Some(("copying_runtime_risk", "copying/runtime risk")),
        _ => None,
    }
}

fn refactor_identity(subject: &str, supporting: &[&Finding]) -> String {
    let identities = supporting
        .iter()
        .filter_map(|finding| (finding.identity != subject).then_some(finding.identity.as_str()))
        .collect::<BTreeSet<_>>();

    if identities.len() == 1 {
        identities.into_iter().next().unwrap_or(subject).to_owned()
    } else {
        subject.to_owned()
    }
}

fn refactor_delta(supporting: &[&Finding]) -> DeltaStatus {
    if supporting
        .iter()
        .any(|finding| finding.delta == DeltaStatus::Worsened)
    {
        DeltaStatus::Worsened
    } else if supporting
        .iter()
        .any(|finding| finding.delta == DeltaStatus::New)
    {
        DeltaStatus::New
    } else if supporting
        .iter()
        .any(|finding| finding.delta == DeltaStatus::Current)
    {
        DeltaStatus::Current
    } else if supporting
        .iter()
        .any(|finding| finding.delta == DeltaStatus::Unchanged)
    {
        DeltaStatus::Unchanged
    } else {
        DeltaStatus::Unknown
    }
}

fn refactor_direction(signals: BTreeSet<&str>) -> String {
    let complexity = signals.contains("decision_complexity");
    let dependency = signals.contains("dependency_surface");
    let copying = signals.contains("copying_runtime_risk");

    match (complexity, dependency, copying) {
        (true, true, true) => "consider splitting responsibilities and narrowing dependency surface; isolate copying-sensitive paths and measure runtime/build effects before changing behavior".into(),
        (true, true, false) => "consider splitting responsibilities and narrowing dependency surface without prescribing a final architecture".into(),
        (true, false, true) => "consider separating complex orchestration from copying-sensitive paths; inspect receiver types and execution frequency before changing behavior".into(),
        (false, true, true) => "consider isolating copying-sensitive behavior behind a narrower, stable dependency boundary; measure runtime and build impact before optimizing".into(),
        _ => "investigate the corroborating evidence before choosing a refactor direction".into(),
    }
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
    let (&first, rest) = values.split_first()?;
    let mut max = first;
    let mut max_index = 0usize;
    let mut max_count = 1usize;

    for (index, &value) in rest.iter().enumerate() {
        if value > max {
            max = value;
            max_index = index + 1;
            max_count = 1;
        } else if value == max {
            max_count += 1;
        }
    }

    if max_count != 1 {
        return None;
    }

    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let median = sorted[sorted.len() / 2];
    if max <= median {
        return None;
    }

    Some((max_index, max, median))
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
