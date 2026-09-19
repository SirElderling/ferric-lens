    use std::{
        collections::BTreeSet,
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::{
        collect_reachable_module, inventory_from_metadata, module_path_from_relative, rust_name,
        Metadata, Package, SourceBudget, Target, MAX_SNAPSHOT_SOURCE_BYTES,
    };

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_root() -> PathBuf {
        let counter = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "ferric-lens-input-test-{}-{counter}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).unwrap();
        root.canonicalize().unwrap()
    }

    #[test]
    fn derives_module_paths_from_common_layouts() {
        assert_eq!(module_path_from_relative("src/lib.rs"), "");
        assert_eq!(module_path_from_relative("src/foo.rs"), "foo");
        assert_eq!(module_path_from_relative("src/foo/mod.rs"), "foo");
        assert_eq!(module_path_from_relative("src/foo/bar.rs"), "foo::bar");
        assert_eq!(module_path_from_relative("crates/a/src/x.rs"), "x");
    }

    #[test]
    fn source_budget_stops_at_the_snapshot_limit() {
        let mut budget = SourceBudget::default();
        assert!(budget.reserve(MAX_SNAPSHOT_SOURCE_BYTES));
        assert!(!budget.reserve(1));
        assert!(budget.exhausted);
    }

    #[test]
    fn target_inventory_excludes_orphan_rust_files() {
        let root = temp_root();
        fs::write(root.join("src/lib.rs"), "mod used;").unwrap();
        fs::write(root.join("src/used.rs"), "pub fn used() {}").unwrap();
        fs::write(root.join("src/orphan.rs"), "pub fn orphan() {}").unwrap();

        let mut visited = BTreeSet::new();
        let mut sources = Vec::new();
        let mut limitations = Vec::new();
        let mut budget = SourceBudget::default();
        collect_reachable_module(
            &root,
            "demo",
            &root.join("src/lib.rs"),
            "",
            true,
            None,
            &mut visited,
            &mut budget,
            &mut sources,
            &mut limitations,
        )
        .unwrap();

        sources.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        assert_eq!(
            sources
                .iter()
                .map(|source| source.relative_path.as_str())
                .collect::<Vec<_>>(),
            vec!["src/lib.rs", "src/used.rs"]
        );
        assert!(limitations.is_empty());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn discovers_file_modules_nested_under_inline_modules() {
        let root = temp_root();
        fs::create_dir_all(root.join("src/outer")).unwrap();
        fs::write(root.join("src/lib.rs"), "mod outer { mod child; }").unwrap();
        fs::write(root.join("src/outer/child.rs"), "pub fn child() {}").unwrap();

        let mut visited = BTreeSet::new();
        let mut sources = Vec::new();
        let mut limitations = Vec::new();
        let mut budget = SourceBudget::default();
        collect_reachable_module(
            &root,
            "demo",
            &root.join("src/lib.rs"),
            "",
            true,
            None,
            &mut visited,
            &mut budget,
            &mut sources,
            &mut limitations,
        )
        .unwrap();

        assert!(sources.iter().any(|source| {
            source.relative_path == "src/outer/child.rs" && source.module_path == "outer::child"
        }));
        assert!(limitations.is_empty());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn library_and_binary_with_same_target_name_remain_distinct() {
        let root = temp_root();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname='demo'\nversion='0.1.0'\n",
        )
        .unwrap();
        fs::write(root.join("src/lib.rs"), "pub fn library() {}").unwrap();
        fs::write(
            root.join("src/main.rs"),
            "use demo::library; fn main() { library(); }",
        )
        .unwrap();

        let package_id = "path+file:///demo#0.1.0".to_owned();
        let metadata = Metadata {
            packages: vec![Package {
                name: "demo".into(),
                id: package_id.clone(),
                manifest_path: root.join("Cargo.toml").to_string_lossy().into_owned(),
                targets: vec![
                    Target {
                        name: "demo".into(),
                        kind: vec!["lib".into()],
                        src_path: root.join("src/lib.rs").to_string_lossy().into_owned(),
                    },
                    Target {
                        name: "demo".into(),
                        kind: vec!["bin".into()],
                        src_path: root.join("src/main.rs").to_string_lossy().into_owned(),
                    },
                ],
                dependencies: Vec::new(),
            }],
            workspace_members: vec![package_id],
            workspace_root: root.to_string_lossy().into_owned(),
        };

        let (sources, aliases, limitations) =
            inventory_from_metadata(&root, metadata, None).unwrap();

        assert!(limitations.is_empty());
        assert!(sources
            .iter()
            .any(|source| source.crate_name == "demo[lib]"));
        assert!(sources
            .iter()
            .any(|source| source.crate_name == "demo[bin]"));
        assert_eq!(
            aliases
                .get("demo[bin]")
                .and_then(|aliases| aliases.get("demo")),
            Some(&"demo[lib]".to_owned())
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn normalizes_cargo_names_for_rust_imports() {
        assert_eq!(rust_name("jeko-core"), "jeko_core");
        assert_eq!(rust_name("custom_name"), "custom_name");
    }

    #[test]
    fn source_budget_rejects_exhausted_and_overflowed_reservations() {
        let mut exhausted = SourceBudget {
            used: 0,
            exhausted: true,
        };
        assert!(!exhausted.reserve(1));

        let mut overflow = SourceBudget {
            used: u64::MAX,
            exhausted: false,
        };
        assert!(!overflow.reserve(1));
        assert!(overflow.exhausted);
    }

    #[test]
    fn public_inventory_falls_back_when_cargo_metadata_is_unavailable() {
        let root = temp_root();
        fs::write(root.join("src/lib.rs"), "pub fn fallback() {}").unwrap();

        let inventory = super::inventory(&root).unwrap();

        assert!(!inventory.metadata_complete);
        assert_eq!(inventory.sources.len(), 1);
        assert_eq!(inventory.sources[0].relative_path, "src/lib.rs");
        assert!(inventory
            .metadata_detail
            .as_deref()
            .unwrap()
            .contains("Cargo metadata unavailable"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fallback_inventory_excludes_auxiliary_trees_and_non_rust_files() {
        let root = temp_root();
        for directory in [
            "tests",
            "benches",
            "examples",
            ".git",
            ".ferric-lens",
            "target",
            "src/nested",
        ] {
            fs::create_dir_all(root.join(directory)).unwrap();
        }
        fs::write(root.join("src/lib.rs"), "pub fn root() {}").unwrap();
        fs::write(root.join("src/nested/kept.rs"), "pub fn kept() {}").unwrap();
        fs::write(root.join("src/readme.txt"), "ignore").unwrap();
        for path in [
            "tests/ignored.rs",
            "benches/ignored.rs",
            "examples/ignored.rs",
            ".git/ignored.rs",
            ".ferric-lens/ignored.rs",
            "target/ignored.rs",
        ] {
            fs::write(root.join(path), "pub fn ignored() {}").unwrap();
        }

        let (mut sources, limitations) = super::fallback_inventory(&root).unwrap();
        sources.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));

        assert_eq!(
            sources
                .iter()
                .map(|source| source.relative_path.as_str())
                .collect::<Vec<_>>(),
            ["src/lib.rs", "src/nested/kept.rs"]
        );
        assert!(limitations.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn fallback_inventory_skips_symlinks() {
        use std::os::unix::fs::symlink;

        let root = temp_root();
        fs::write(root.join("src/real.rs"), "pub fn real() {}").unwrap();
        symlink(root.join("src/real.rs"), root.join("src/link.rs")).unwrap();

        let (sources, limitations) = super::fallback_inventory(&root).unwrap();

        assert_eq!(sources.len(), 1);
        assert!(limitations
            .iter()
            .any(|detail| detail.contains("fallback inventory skipped symlink")));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fallback_inventory_skips_oversized_source_files() {
        use std::fs::File;

        let root = temp_root();
        let file = File::create(root.join("src/huge.rs")).unwrap();
        file.set_len(super::MAX_SOURCE_FILE_BYTES + 1).unwrap();

        let (sources, limitations) = super::fallback_inventory(&root).unwrap();

        assert!(sources.is_empty());
        assert!(limitations
            .iter()
            .any(|detail| detail.contains("exceeds the 8 MiB source limit")));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reachable_module_reports_missing_symlink_oversized_external_and_duplicate_sources() {
        use std::fs::File;

        let root = temp_root();
        let mut visited = BTreeSet::new();
        let mut sources = Vec::new();
        let mut limitations = Vec::new();
        let mut budget = SourceBudget::default();

        collect_reachable_module(
            &root,
            "demo",
            &root.join("src/missing.rs"),
            "missing",
            false,
            None,
            &mut visited,
            &mut budget,
            &mut sources,
            &mut limitations,
        )
        .unwrap();

        let huge = root.join("src/huge.rs");
        File::create(&huge)
            .unwrap()
            .set_len(super::MAX_SOURCE_FILE_BYTES + 1)
            .unwrap();
        collect_reachable_module(
            &root,
            "demo",
            &huge,
            "huge",
            false,
            None,
            &mut visited,
            &mut budget,
            &mut sources,
            &mut limitations,
        )
        .unwrap();

        let outside = std::env::temp_dir().join(format!(
            "ferric-lens-outside-{}-{}.rs",
            std::process::id(),
            TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&outside, "pub fn outside() {}").unwrap();
        collect_reachable_module(
            &root,
            "demo",
            &outside,
            "outside",
            false,
            None,
            &mut visited,
            &mut budget,
            &mut sources,
            &mut limitations,
        )
        .unwrap();

        let normal = root.join("src/normal.rs");
        fs::write(&normal, "pub fn normal() {}").unwrap();
        collect_reachable_module(
            &root,
            "demo",
            &normal,
            "normal",
            false,
            None,
            &mut visited,
            &mut budget,
            &mut sources,
            &mut limitations,
        )
        .unwrap();
        collect_reachable_module(
            &root,
            "demo",
            &normal,
            "normal",
            false,
            None,
            &mut visited,
            &mut budget,
            &mut sources,
            &mut limitations,
        )
        .unwrap();

        assert_eq!(sources.len(), 1);
        assert!(limitations
            .iter()
            .any(|detail| detail.contains("is unavailable")));
        assert!(limitations
            .iter()
            .any(|detail| detail.contains("exceeds the 8 MiB source limit")));
        assert!(limitations
            .iter()
            .any(|detail| detail.contains("resolves outside the repository")));

        fs::remove_file(outside).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn reachable_module_rejects_symlink_sources() {
        use std::os::unix::fs::symlink;

        let root = temp_root();
        fs::write(root.join("src/real.rs"), "pub fn real() {}").unwrap();
        symlink(root.join("src/real.rs"), root.join("src/link.rs")).unwrap();
        let mut visited = BTreeSet::new();
        let mut sources = Vec::new();
        let mut limitations = Vec::new();
        let mut budget = SourceBudget::default();

        collect_reachable_module(
            &root,
            "demo",
            &root.join("src/link.rs"),
            "link",
            false,
            None,
            &mut visited,
            &mut budget,
            &mut sources,
            &mut limitations,
        )
        .unwrap();

        assert!(sources.is_empty());
        assert!(limitations
            .iter()
            .any(|detail| detail.contains("is a symlink and was not followed")));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reachable_module_reports_non_utf8_and_invalid_rust_without_dropping_source_bytes() {
        for (name, bytes, expected) in [
            ("binary.rs", vec![0xff], "not UTF-8"),
            (
                "broken.rs",
                b"fn {".to_vec(),
                "cannot discover child modules",
            ),
        ] {
            let root = temp_root();
            fs::write(root.join("src").join(name), bytes).unwrap();
            let mut visited = BTreeSet::new();
            let mut sources = Vec::new();
            let mut limitations = Vec::new();
            let mut budget = SourceBudget::default();

            collect_reachable_module(
                &root,
                "demo",
                &root.join("src").join(name),
                "broken",
                false,
                None,
                &mut visited,
                &mut budget,
                &mut sources,
                &mut limitations,
            )
            .unwrap();

            assert_eq!(sources.len(), 1);
            assert!(limitations.iter().any(|detail| detail.contains(expected)));
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn reachable_module_honors_an_exhausted_aggregate_budget() {
        let root = temp_root();
        fs::write(root.join("src/lib.rs"), "pub fn root() {}").unwrap();
        let mut visited = BTreeSet::new();
        let mut sources = Vec::new();
        let mut limitations = Vec::new();
        let mut budget = SourceBudget {
            used: super::MAX_SNAPSHOT_SOURCE_BYTES,
            exhausted: false,
        };

        collect_reachable_module(
            &root,
            "demo",
            &root.join("src/lib.rs"),
            "",
            true,
            None,
            &mut visited,
            &mut budget,
            &mut sources,
            &mut limitations,
        )
        .unwrap();

        assert!(sources.is_empty());
        assert!(limitations
            .iter()
            .any(|detail| detail.contains("aggregate production source input")));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn module_discovery_reports_cfg_path_missing_and_ambiguous_sources() {
        use crate::{cfg::HostCfg, model::AnalysisProfile, profile::ProfileContext};

        let root = temp_root();
        fs::create_dir_all(root.join("src/ambiguous")).unwrap();
        fs::write(
            root.join("src/lib.rs"),
            r#"
                #[cfg_attr(unix, path = "x.rs")]
                mod cfg_attr_mod;
                #[cfg(custom_flag)]
                mod unknown_cfg;
                #[cfg(windows)]
                mod disabled;
                #[path = "custom.rs"]
                mod custom_path;
                mod missing;
                mod ambiguous;
            "#,
        )
        .unwrap();
        fs::write(root.join("src/ambiguous.rs"), "pub fn a() {}").unwrap();
        fs::write(root.join("src/ambiguous/mod.rs"), "pub fn nested() {}").unwrap();

        let profile = ProfileContext {
            public: AnalysisProfile {
                id: "test".into(),
                target: "host".into(),
                resolved_target: "test-target".into(),
                features: Vec::new(),
                target_cfg: Vec::new(),
            },
            cfg: HostCfg::test(&["unix", "target_os=\"linux\""]),
        };
        let mut visited = BTreeSet::new();
        let mut sources = Vec::new();
        let mut limitations = Vec::new();
        let mut budget = SourceBudget::default();

        collect_reachable_module(
            &root,
            "demo",
            &root.join("src/lib.rs"),
            "",
            true,
            Some(&profile),
            &mut visited,
            &mut budget,
            &mut sources,
            &mut limitations,
        )
        .unwrap();

        assert_eq!(sources.len(), 1);
        for expected in [
            "uses cfg_attr",
            "unresolved cfg reachability",
            "uses #[path]",
            "has no discoverable source file",
            "has ambiguous source files",
        ] {
            assert!(
                limitations.iter().any(|detail| detail.contains(expected)),
                "missing limitation: {expected}; got {limitations:?}"
            );
        }
        assert!(!limitations.iter().any(|detail| detail.contains("disabled")));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn module_discovery_supports_nested_mod_rs_and_test_cfg_is_excluded_when_cfg_is_unavailable() {
        use crate::{cfg::HostCfg, model::AnalysisProfile, profile::ProfileContext};

        let root = temp_root();
        fs::create_dir_all(root.join("src/nested")).unwrap();
        fs::write(
            root.join("src/lib.rs"),
            "#[cfg(test)] mod tests_only; mod nested;",
        )
        .unwrap();
        fs::write(root.join("src/tests_only.rs"), "compile_error!(\"no\");").unwrap();
        fs::write(root.join("src/nested/mod.rs"), "pub fn nested() {}").unwrap();
        let profile = ProfileContext {
            public: AnalysisProfile {
                id: "unavailable".into(),
                target: "host".into(),
                resolved_target: "unknown".into(),
                features: Vec::new(),
                target_cfg: Vec::new(),
            },
            cfg: HostCfg::unavailable(),
        };
        let mut visited = BTreeSet::new();
        let mut sources = Vec::new();
        let mut limitations = Vec::new();
        let mut budget = SourceBudget::default();

        collect_reachable_module(
            &root,
            "demo",
            &root.join("src/lib.rs"),
            "",
            true,
            Some(&profile),
            &mut visited,
            &mut budget,
            &mut sources,
            &mut limitations,
        )
        .unwrap();

        assert!(sources
            .iter()
            .any(|source| source.relative_path == "src/nested/mod.rs"));
        assert!(!sources
            .iter()
            .any(|source| source.relative_path == "src/tests_only.rs"));
        assert!(limitations.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn workspace_metadata_builds_renamed_dependency_aliases_and_skips_non_production_targets() {
        let root = temp_root();
        let crate_a = root.join("crates/a");
        let crate_b = root.join("crates/b");
        fs::create_dir_all(crate_a.join("src")).unwrap();
        fs::create_dir_all(crate_b.join("src")).unwrap();
        fs::write(
            crate_a.join("Cargo.toml"),
            "[package]\nname='a'\nversion='0.1.0'\n",
        )
        .unwrap();
        fs::write(
            crate_b.join("Cargo.toml"),
            "[package]\nname='b'\nversion='0.1.0'\n",
        )
        .unwrap();
        fs::write(crate_a.join("src/lib.rs"), "pub fn a() {}").unwrap();
        fs::write(crate_b.join("src/lib.rs"), "pub fn b() {}").unwrap();
        fs::write(crate_a.join("src/ignored.rs"), "pub fn ignored() {}").unwrap();

        let a_id = "a-id".to_owned();
        let b_id = "b-id".to_owned();
        let metadata = Metadata {
            packages: vec![
                Package {
                    name: "a".into(),
                    id: a_id.clone(),
                    manifest_path: crate_a.join("Cargo.toml").to_string_lossy().into_owned(),
                    targets: vec![
                        Target {
                            name: "a".into(),
                            kind: vec!["lib".into()],
                            src_path: crate_a.join("src/lib.rs").to_string_lossy().into_owned(),
                        },
                        Target {
                            name: "ignored-test".into(),
                            kind: vec!["test".into()],
                            src_path: crate_a
                                .join("src/ignored.rs")
                                .to_string_lossy()
                                .into_owned(),
                        },
                    ],
                    dependencies: vec![super::Dependency {
                        name: "b".into(),
                        rename: Some("renamed-b".into()),
                        path: Some(crate_b.to_string_lossy().into_owned()),
                    }],
                },
                Package {
                    name: "b".into(),
                    id: b_id.clone(),
                    manifest_path: crate_b.join("Cargo.toml").to_string_lossy().into_owned(),
                    targets: vec![Target {
                        name: "b".into(),
                        kind: vec!["rlib".into()],
                        src_path: crate_b.join("src/lib.rs").to_string_lossy().into_owned(),
                    }],
                    dependencies: Vec::new(),
                },
            ],
            workspace_members: vec![a_id, b_id],
            workspace_root: root.to_string_lossy().into_owned(),
        };

        let (sources, aliases, limitations) =
            inventory_from_metadata(&root, metadata, None).unwrap();

        assert!(limitations.is_empty());
        assert_eq!(sources.len(), 2);
        assert_eq!(
            aliases.get("a").and_then(|values| values.get("renamed_b")),
            Some(&"b".to_owned())
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn limitation_summaries_are_deterministic_and_bounded() {
        let limitations = vec![
            "one".to_owned(),
            "two".to_owned(),
            "three".to_owned(),
            "four".to_owned(),
        ];

        assert_eq!(
            super::summarize_limitations(&limitations).as_deref(),
            Some("one; two; three; 1 additional inventory limitation(s)")
        );
        assert_eq!(
            super::fallback_detail("primary", &limitations).as_deref(),
            Some("primary; one; two; three; 1 additional inventory limitation(s)")
        );
        assert_eq!(super::summarize_limitations(&[]), None);
        assert_eq!(
            super::fallback_detail("primary", &[]).as_deref(),
            Some("primary")
        );
    }

    #[test]
    fn path_helpers_handle_non_src_and_nested_paths() {
        assert_eq!(module_path_from_relative("foo.rs"), "foo");
        assert_eq!(module_path_from_relative("main.rs"), "");
        assert_eq!(module_path_from_relative("a/b/mod.rs"), "a::b");
        assert_eq!(
            super::slash_path(std::path::Path::new("a").join("b").as_path()),
            "a/b"
        );
    }
