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
        let baseline = vec![module("src/a.rs", "a", "a"), module("src/b.rs", "b", "b")];
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
