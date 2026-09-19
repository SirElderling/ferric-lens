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


#[test]
fn ignores_dependencies_outside_the_observed_module_graph() {
    let summary = summarize(&[module("a", &["external::missing", "demo::a"])]);

    assert_eq!(summary.modules, 1);
    assert_eq!(summary.explicit_dependency_edges, 0);
    assert!(summary.cycles.is_empty());
}
