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
mod tests {
    use super::match_modules;
    use crate::{git::ChangeSet, model::ModuleMetrics};

    fn module(path: &str, module_path: &str, digest: &str) -> ModuleMetrics {
        ModuleMetrics {
            crate_name: "demo".into(),
            module_path: module_path.into(),
            path: path.into(),
            lines: 1,
            decision_sites: 0,
            public_items: 0,
            explicit_imports: Vec::new(),
            local_dependency_modules: Vec::new(),
            structure_digest: digest.into(),
            parse_complete: true,
            gate_complete: true,
            limitation: None,
            history: None,
        }
    }

    #[test]
    fn stable_identity_wins() {
        let baseline = vec![module("src/a.rs", "a", "old")];
        let head = vec![module("src/a.rs", "a", "new")];
        let matched = match_modules(&baseline, &head, &ChangeSet::default());
        assert_eq!(matched.head_to_baseline.get(&0), Some(&0));
    }

    #[test]
    fn git_rename_matches_modified_module() {
        let baseline = vec![module("src/a.rs", "a", "old")];
        let head = vec![module("src/b.rs", "b", "new")];
        let mut changes = ChangeSet::default();
        changes.renames.insert("src/a.rs".into(), "src/b.rs".into());
        let matched = match_modules(&baseline, &head, &changes);
        assert_eq!(matched.head_to_baseline.get(&0), Some(&0));
    }

    #[test]
    fn unique_structure_matches_formatting_or_container_moves() {
        let baseline = vec![module("src/a.rs", "a", "same")];
        let head = vec![module("src/nested/a.rs", "nested::a", "same")];
        let matched = match_modules(&baseline, &head, &ChangeSet::default());
        assert_eq!(matched.head_to_baseline.get(&0), Some(&0));
    }

    #[test]
    fn duplicate_structure_is_not_guessed() {
        let baseline = vec![
            module("src/a.rs", "a", "same"),
            module("src/b.rs", "b", "same"),
        ];
        let head = vec![module("src/c.rs", "c", "same")];
        let matched = match_modules(&baseline, &head, &ChangeSet::default());
        assert!(!matched.head_to_baseline.contains_key(&0));
        assert!(matched.ambiguous_head.contains(&0));
    }

    #[test]
    fn rename_matching_skips_missing_conflicting_and_cross_crate_edges() {
        let baseline = vec![
            module("src/a.rs", "a", "a"),
            module("src/b.rs", "b", "b"),
        ];
        let mut cross = module("src/c.rs", "c", "c");
        cross.crate_name = "other".into();
        let head = vec![
            module("src/a.rs", "a", "changed"),
            module("src/renamed.rs", "renamed", "different"),
            cross,
        ];
        let mut changes = ChangeSet::default();
        changes
            .renames
            .insert("src/missing.rs".into(), "src/nowhere.rs".into());
        changes
            .renames
            .insert("src/a.rs".into(), "src/renamed.rs".into());
        changes.renames.insert("src/b.rs".into(), "src/c.rs".into());

        let matched = match_modules(&baseline, &head, &changes);

        assert_eq!(matched.head_to_baseline.get(&0), Some(&0));
        assert!(!matched.head_to_baseline.contains_key(&1));
        assert!(!matched.head_to_baseline.contains_key(&2));
    }

    #[test]
    fn empty_and_unmatched_structure_digests_are_not_correspondence() {
        let baseline = vec![
            module("src/a.rs", "a", ""),
            module("src/b.rs", "b", "baseline-only"),
        ];
        let head = vec![
            module("src/c.rs", "c", ""),
            module("src/d.rs", "d", "head-only"),
        ];

        let matched = match_modules(&baseline, &head, &ChangeSet::default());

        assert!(matched.head_to_baseline.is_empty());
        assert!(matched.ambiguous_head.is_empty());
    }

}
