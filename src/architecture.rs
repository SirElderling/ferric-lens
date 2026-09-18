use std::collections::{BTreeMap, BTreeSet};

use crate::model::{ArchitectureSummary, DependencyCycle, ModuleMetrics};

pub fn summarize(modules: &[ModuleMetrics]) -> ArchitectureSummary {
    let nodes = modules
        .iter()
        .filter(|module| module.parse_complete)
        .map(|module| subject(&module.crate_name, &module.module_path))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let index = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.as_str(), index))
        .collect::<BTreeMap<_, _>>();

    let mut adjacency = vec![Vec::<usize>::new(); nodes.len()];
    for module in modules.iter().filter(|module| module.parse_complete) {
        let source_name = subject(&module.crate_name, &module.module_path);
        let Some(&source) = index.get(source_name.as_str()) else {
            continue;
        };

        for dependency in &module.local_dependency_modules {
            let Some(&target) = index.get(dependency.as_str()) else {
                continue;
            };
            if source != target {
                adjacency[source].push(target);
            }
        }
    }

    for edges in &mut adjacency {
        edges.sort_unstable();
        edges.dedup();
    }

    let explicit_dependency_edges = adjacency.iter().map(Vec::len).sum();
    let cycles = strongly_connected_cycles(&nodes, &adjacency);

    ArchitectureSummary {
        modules: nodes.len(),
        explicit_dependency_edges,
        incomplete_modules: modules
            .iter()
            .filter(|module| module.parse_complete && !module.gate_complete)
            .count(),
        cycles,
    }
}

fn strongly_connected_cycles(nodes: &[String], adjacency: &[Vec<usize>]) -> Vec<DependencyCycle> {
    let finish = finish_order(adjacency);
    let mut reverse = vec![Vec::<usize>::new(); adjacency.len()];
    for (source, targets) in adjacency.iter().enumerate() {
        for &target in targets {
            reverse[target].push(source);
        }
    }
    for edges in &mut reverse {
        edges.sort_unstable();
        edges.dedup();
    }

    let mut assigned = vec![false; adjacency.len()];
    let mut cycles = Vec::new();

    for start in finish.into_iter().rev() {
        if assigned[start] {
            continue;
        }

        assigned[start] = true;
        let mut stack = vec![start];
        let mut component = Vec::new();

        while let Some(node) = stack.pop() {
            component.push(node);
            for &next in reverse[node].iter().rev() {
                if !assigned[next] {
                    assigned[next] = true;
                    stack.push(next);
                }
            }
        }

        if component.len() > 1 {
            let mut module_names = component
                .into_iter()
                .map(|index| nodes[index].clone())
                .collect::<Vec<_>>();
            module_names.sort();
            cycles.push(DependencyCycle {
                modules: module_names,
            });
        }
    }

    cycles.sort_by(|left, right| left.modules.cmp(&right.modules));
    cycles
}

fn finish_order(adjacency: &[Vec<usize>]) -> Vec<usize> {
    let mut visited = vec![false; adjacency.len()];
    let mut order = Vec::with_capacity(adjacency.len());

    for start in 0..adjacency.len() {
        if visited[start] {
            continue;
        }

        visited[start] = true;
        let mut stack = vec![(start, 0usize)];

        while let Some((node, next_index)) = stack.pop() {
            if next_index < adjacency[node].len() {
                stack.push((node, next_index + 1));
                let next = adjacency[node][next_index];
                if !visited[next] {
                    visited[next] = true;
                    stack.push((next, 0));
                }
            } else {
                order.push(node);
            }
        }
    }

    order
}

fn subject(crate_name: &str, module_path: &str) -> String {
    if module_path.is_empty() {
        crate_name.to_owned()
    } else {
        format!("{crate_name}::{module_path}")
    }
}

#[cfg(test)]
mod tests {
    use crate::model::ModuleMetrics;

    use super::summarize;

    fn module(name: &str, dependencies: &[&str]) -> ModuleMetrics {
        ModuleMetrics {
            crate_name: "demo".into(),
            module_path: name.into(),
            path: format!("src/{name}.rs"),
            lines: 1,
            decision_sites: 0,
            public_items: 0,
            explicit_imports: Vec::new(),
            local_dependency_modules: dependencies.iter().map(|value| (*value).into()).collect(),
            structure_digest: name.into(),
            parse_complete: true,
            gate_complete: true,
            limitation: None,
            history: None,
        }
    }

    #[test]
    fn finds_deterministic_explicit_import_cycles() {
        let modules = vec![
            module("a", &["demo::b"]),
            module("b", &["demo::a"]),
            module("c", &["demo::a"]),
        ];

        let summary = summarize(&modules);

        assert_eq!(summary.modules, 3);
        assert_eq!(summary.explicit_dependency_edges, 3);
        assert_eq!(summary.cycles.len(), 1);
        assert_eq!(summary.cycles[0].modules, ["demo::a", "demo::b"]);
    }

    #[test]
    fn reports_incomplete_graph_coverage_without_inventing_cycles() {
        let mut incomplete = module("a", &[]);
        incomplete.gate_complete = false;
        let summary = summarize(&[incomplete, module("b", &[])]);

        assert_eq!(summary.incomplete_modules, 1);
        assert!(summary.cycles.is_empty());
    }
}
