use std::collections::BTreeMap;

use crate::model::{Evidence, EvidenceClass, Finding, ModuleMetrics, Priority};

const MIN_POPULATION: usize = 20;

/// Produce advisory current-snapshot findings.
///
/// These findings deliberately cannot affect the gate verdict. The actual V1
/// blocking rule requires a comparable Git baseline and material-growth
/// predicates, which are implemented in a later slice.
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
                    subject: format!("{crate_name}::{}", module.module_path),
                    evidence_class: EvidenceClass::Strong,
                    priority: Priority::Investigate,
                    summary: "module is simultaneously an outlier for decision sites and local dependency breadth".into(),
                    direction: "investigate whether responsibility and dependency surface can be reduced without changing behavior".into(),
                    evidence: vec![
                        Evidence {
                            metric: "decision_sites".into(),
                            value: module.decision_sites,
                            reference: decision_p90,
                            population: decision_values.len(),
                        },
                        Evidence {
                            metric: "local_dependency_modules".into(),
                            value: dependencies,
                            reference: dependency_p90,
                            population: dependency_values.len(),
                        },
                    ],
                });
            }
        }
    }

    findings.sort_by(|a, b| (&a.rule, &a.subject).cmp(&(&b.rule, &b.subject)));
    findings
}

fn nearest_rank_p90(values: &[usize]) -> usize {
    debug_assert!(!values.is_empty());
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let rank = (9 * sorted.len() + 9) / 10;
    sorted[rank.saturating_sub(1)]
}

#[cfg(test)]
mod tests {
    use super::nearest_rank_p90;

    #[test]
    fn uses_nearest_rank_p90() {
        let values = (1..=20).collect::<Vec<_>>();
        assert_eq!(nearest_rank_p90(&values), 18);
    }
}
