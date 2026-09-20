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
        let source = index[source_name.as_str()];

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
#[path = "architecture_tests.rs"]
mod tests;
