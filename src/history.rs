use std::collections::{BTreeMap, BTreeSet};

use crate::{
    git::HistorySample,
    model::{CoChangeEvidence, HistoryEvidence, HistorySummary, ModuleMetrics},
};

const MAX_REPORTED_COCHANGE: usize = 5;
const MAX_COCHANGE_PATHS_PER_COMMIT: usize = 200;

pub fn enrich(
    modules: &mut [ModuleMetrics],
    sample: &HistorySample,
    candidates: &BTreeSet<String>,
) -> HistorySummary {
    let mut change_counts = BTreeMap::<String, usize>::new();
    let mut cochange_counts = BTreeMap::<String, BTreeMap<String, usize>>::new();

    for commit in &sample.commits {
        let unique_paths = commit.paths.iter().cloned().collect::<BTreeSet<_>>();
        let touched_candidates = unique_paths
            .iter()
            .filter(|path| candidates.contains(path.as_str()))
            .cloned()
            .collect::<Vec<_>>();

        for candidate in &touched_candidates {
            *change_counts.entry(candidate.clone()).or_default() += 1;
        }

        if unique_paths.len() > MAX_COCHANGE_PATHS_PER_COMMIT {
            continue;
        }

        for candidate in touched_candidates {
            let counts = cochange_counts.entry(candidate.clone()).or_default();
            for other in &unique_paths {
                if other != &candidate {
                    *counts.entry(other.clone()).or_default() += 1;
                }
            }
        }
    }

    for module in modules {
        if !candidates.contains(&module.path) {
            continue;
        }

        let mut cochange = cochange_counts
            .remove(&module.path)
            .unwrap_or_default()
            .into_iter()
            .map(|(path, shared_commits)| CoChangeEvidence {
                path,
                shared_commits,
            })
            .collect::<Vec<_>>();
        cochange.sort_by(|left, right| {
            right
                .shared_commits
                .cmp(&left.shared_commits)
                .then_with(|| left.path.cmp(&right.path))
        });
        cochange.truncate(MAX_REPORTED_COCHANGE);

        module.history = Some(HistoryEvidence {
            change_commits: change_counts.remove(&module.path).unwrap_or(0),
            sampled_commits: sample.commits.len(),
            cochange,
        });
    }

    HistorySummary {
        sampled_commits: sample.commits.len(),
        changed_path_records: sample.changed_path_records,
        broad_commits_excluded_from_cochange: sample.broad_commits_excluded_from_cochange,
        truncated: sample.truncated,
    }
}

#[cfg(test)]
#[path = "history_tests.rs"]
mod tests;
