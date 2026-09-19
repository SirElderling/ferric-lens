use std::collections::BTreeSet;

use crate::{
    git::{HistoryCommit, HistorySample},
    model::ModuleMetrics,
};

use super::enrich;

fn module(path: &str) -> ModuleMetrics {
    ModuleMetrics {
        crate_name: "demo".into(),
        module_path: path.replace("src/", "").replace(".rs", ""),
        path: path.into(),
        lines: 1,
        decision_sites: 0,
        public_items: 0,
        clone_calls: 0,
        functions: Vec::new(),
        types: Vec::new(),
        explicit_imports: Vec::new(),
        local_dependency_modules: Vec::new(),
        structure_digest: "digest".into(),
        parse_complete: true,
        gate_complete: true,
        limitation: None,
        history: None,
    }
}

#[test]
fn records_churn_and_top_cochange_for_candidate_paths() {
    let sample = HistorySample {
        commits: vec![
            HistoryCommit {
                oid: "1".repeat(40),
                paths: vec!["src/a.rs".into(), "src/b.rs".into()],
            },
            HistoryCommit {
                oid: "2".repeat(40),
                paths: vec!["src/a.rs".into(), "src/c.rs".into()],
            },
        ],
        changed_path_records: 4,
        broad_commits_excluded_from_cochange: 0,
        truncated: false,
    };
    let candidates = BTreeSet::from(["src/a.rs".to_owned()]);
    let mut modules = vec![module("src/a.rs"), module("src/b.rs")];

    let summary = enrich(&mut modules, &sample, &candidates);
    let history = modules[0].history.as_ref().unwrap();

    assert_eq!(summary.sampled_commits, 2);
    assert_eq!(history.change_commits, 2);
    assert_eq!(history.cochange.len(), 2);
    assert_eq!(history.cochange[0].shared_commits, 1);
    assert!(modules[1].history.is_none());
}

#[test]
fn broad_commits_count_churn_but_skip_cochange_pairs() {
    let mut paths = vec!["src/a.rs".to_owned()];
    paths
        .extend((0..=super::MAX_COCHANGE_PATHS_PER_COMMIT).map(|index| format!("src/x{index}.rs")));
    let sample = HistorySample {
        commits: vec![HistoryCommit {
            oid: "a".repeat(40),
            paths,
        }],
        changed_path_records: super::MAX_COCHANGE_PATHS_PER_COMMIT + 2,
        broad_commits_excluded_from_cochange: 1,
        truncated: false,
    };
    let candidates = BTreeSet::from(["src/a.rs".to_owned()]);
    let mut modules = vec![module("src/a.rs")];

    let summary = enrich(&mut modules, &sample, &candidates);

    let history = modules[0].history.as_ref().unwrap();
    assert_eq!(history.change_commits, 1);
    assert!(history.cochange.is_empty());
    assert_eq!(summary.broad_commits_excluded_from_cochange, 1);
}
