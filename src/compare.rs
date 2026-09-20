use std::collections::{BTreeMap, BTreeSet};

use crate::{git::ChangeSet, model::ModuleMetrics};

#[derive(Debug, Default)]
pub struct Correspondence {
    pub head_to_baseline: BTreeMap<usize, usize>,
    pub ambiguous_head: BTreeSet<usize>,
}

pub fn match_modules(
    baseline: &[ModuleMetrics],
    head: &[ModuleMetrics],
    changes: &ChangeSet,
) -> Correspondence {
    let mut result = Correspondence::default();
    let mut used_baseline = BTreeSet::new();

    let baseline_by_identity = baseline
        .iter()
        .enumerate()
        .map(|(index, module)| {
            (
                (module.crate_name.as_str(), module.module_path.as_str()),
                index,
            )
        })
        .collect::<BTreeMap<_, _>>();

    for (head_index, module) in head.iter().enumerate() {
        if let Some(&baseline_index) =
            baseline_by_identity.get(&(module.crate_name.as_str(), module.module_path.as_str()))
        {
            result.head_to_baseline.insert(head_index, baseline_index);
            used_baseline.insert(baseline_index);
        }
    }

    let baseline_by_path = baseline
        .iter()
        .enumerate()
        .map(|(index, module)| (module.path.as_str(), index))
        .collect::<BTreeMap<_, _>>();
    let head_by_path = head
        .iter()
        .enumerate()
        .map(|(index, module)| (module.path.as_str(), index))
        .collect::<BTreeMap<_, _>>();

    for (old, new) in &changes.renames {
        let (Some(&baseline_index), Some(&head_index)) = (
            baseline_by_path.get(old.as_str()),
            head_by_path.get(new.as_str()),
        ) else {
            continue;
        };
        if result.head_to_baseline.contains_key(&head_index)
            || used_baseline.contains(&baseline_index)
            || baseline[baseline_index].crate_name != head[head_index].crate_name
        {
            continue;
        }
        result.head_to_baseline.insert(head_index, baseline_index);
        used_baseline.insert(baseline_index);
    }

    let mut baseline_digests = BTreeMap::<(&str, &str), Vec<usize>>::new();
    for (index, module) in baseline.iter().enumerate() {
        if used_baseline.contains(&index) || module.structure_digest.is_empty() {
            continue;
        }
        baseline_digests
            .entry((&module.crate_name, &module.structure_digest))
            .or_default()
            .push(index);
    }

    let mut head_digests = BTreeMap::<(&str, &str), Vec<usize>>::new();
    for (index, module) in head.iter().enumerate() {
        if result.head_to_baseline.contains_key(&index) || module.structure_digest.is_empty() {
            continue;
        }
        head_digests
            .entry((&module.crate_name, &module.structure_digest))
            .or_default()
            .push(index);
    }

    for (key, head_indices) in head_digests {
        let Some(baseline_indices) = baseline_digests.get(&key) else {
            continue;
        };
        if head_indices.len() == 1 && baseline_indices.len() == 1 {
            let head_index = head_indices[0];
            let baseline_index = baseline_indices[0];
            result.head_to_baseline.insert(head_index, baseline_index);
            used_baseline.insert(baseline_index);
        } else {
            result.ambiguous_head.extend(head_indices);
        }
    }

    result
}

#[cfg(test)]
#[path = "compare_tests.rs"]
mod tests;
