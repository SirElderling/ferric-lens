use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use crate::{cfg::HostCfg, model::AnalysisProfile, profile::ProfileContext};

use super::{
    acquire_inventory, collect_reachable_module, collect_rust_files, collect_target_roots,
    finalize_inventory, inventory_from_metadata, module_path_from_relative, rust_name,
    CargoInputEntry, CargoInputSnapshot, Dependency, Metadata, Package, Resolve, ResolveNode,
    SourceBudget, SourceFile, Target, TargetRoot, WorkspaceAliases, MAX_SNAPSHOT_SOURCE_BYTES,
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
        resolve: None,
    };

    let (sources, aliases, _, limitations) =
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
        Some(&profile.cfg),
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
        "custom.rs is unavailable",
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
        Some(&profile.cfg),
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
        resolve: None,
    };

    let (sources, aliases, _, limitations) =
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

#[test]
fn metadata_inventory_failure_falls_back_without_claiming_completeness() {
    let root = temp_root();
    fs::remove_file(root.join("src/lib.rs")).ok();
    fs::create_dir_all(root.join("src/lib.rs")).unwrap();
    let package_id = "demo-id".to_owned();
    let metadata = Metadata {
        packages: vec![Package {
            name: "demo".into(),
            id: package_id.clone(),
            manifest_path: root.join("Cargo.toml").to_string_lossy().into_owned(),
            targets: vec![Target {
                name: "demo".into(),
                kind: vec!["lib".into()],
                src_path: root.join("src/lib.rs").to_string_lossy().into_owned(),
            }],
            dependencies: Vec::new(),
        }],
        workspace_members: vec![package_id],
        workspace_root: root.to_string_lossy().into_owned(),
        resolve: None,
    };

    let (_, aliases, _, complete, detail) = acquire_inventory(&root, Ok(metadata), None).unwrap();

    assert!(aliases.is_empty());
    assert!(!complete);
    assert!(detail
        .as_deref()
        .unwrap()
        .contains("Cargo metadata inventory failed"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn final_inventory_deduplicates_sources_and_hashes_workspace_aliases() {
    let source = SourceFile {
        crate_name: "demo".into(),
        module_path: "a".into(),
        relative_path: "src/a.rs".into(),
        bytes: b"pub fn a() {}".to_vec(),
    };
    let aliases = WorkspaceAliases::from([(
        "demo".into(),
        BTreeMap::from([("shared".into(), "shared".into())]),
    )]);

    let inventory = finalize_inventory((
        vec![source.clone(), source],
        aliases.clone(),
        BTreeMap::new(),
        true,
        None,
    ));
    let without_aliases = finalize_inventory((
        inventory.sources.clone(),
        WorkspaceAliases::new(),
        BTreeMap::new(),
        true,
        None,
    ));

    assert_eq!(inventory.sources.len(), 1);
    assert_eq!(inventory.workspace_aliases, aliases);
    assert_ne!(inventory.content_digest, without_aliases.content_digest);
}

#[test]
fn metadata_inventory_skips_external_packages_invalid_targets_and_non_workspace_dependencies() {
    let root = temp_root();
    fs::write(root.join("src/lib.rs"), "pub fn demo() {}").unwrap();
    fs::write(root.join("src/not-rust.txt"), "ignored").unwrap();

    let outside = std::env::temp_dir().join(format!(
        "ferric-lens-input-outside-package-{}-{}",
        std::process::id(),
        TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(outside.join("src")).unwrap();
    fs::write(outside.join("src/lib.rs"), "pub fn outside() {}").unwrap();

    let demo_id = "demo-id".to_owned();
    let outside_id = "outside-id".to_owned();
    let metadata = Metadata {
        packages: vec![
            Package {
                name: "demo".into(),
                id: demo_id.clone(),
                manifest_path: root.join("Cargo.toml").to_string_lossy().into_owned(),
                targets: vec![
                    Target {
                        name: "demo".into(),
                        kind: vec!["lib".into()],
                        src_path: root.join("src/lib.rs").to_string_lossy().into_owned(),
                    },
                    Target {
                        name: "bad".into(),
                        kind: vec!["bin".into()],
                        src_path: root.join("src/not-rust.txt").to_string_lossy().into_owned(),
                    },
                ],
                dependencies: vec![
                    Dependency {
                        name: "registry".into(),
                        rename: None,
                        path: None,
                    },
                    Dependency {
                        name: "missing-workspace-lib".into(),
                        rename: None,
                        path: Some(root.join("missing").to_string_lossy().into_owned()),
                    },
                ],
            },
            Package {
                name: "outside".into(),
                id: outside_id.clone(),
                manifest_path: outside.join("Cargo.toml").to_string_lossy().into_owned(),
                targets: vec![Target {
                    name: "outside".into(),
                    kind: vec!["lib".into()],
                    src_path: outside.join("src/lib.rs").to_string_lossy().into_owned(),
                }],
                dependencies: Vec::new(),
            },
        ],
        workspace_members: vec![demo_id, outside_id],
        workspace_root: root.to_string_lossy().into_owned(),
        resolve: None,
    };

    let (sources, aliases, _, limitations) =
        inventory_from_metadata(&root, metadata, None).unwrap();

    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].relative_path, "src/lib.rs");
    assert!(aliases["demo"].is_empty());
    assert!(limitations.is_empty());

    fs::remove_dir_all(outside).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn target_collection_stops_immediately_when_budget_is_exhausted() {
    let root = temp_root();
    fs::write(root.join("src/lib.rs"), "pub fn demo() {}").unwrap();
    let target = TargetRoot {
        id: "demo".into(),
        import_name: "demo".into(),
        kind: "lib",
        source: root.join("src/lib.rs"),
        package_root: root.clone(),
        dependencies: Vec::new(),
        resolved_features: None,
    };
    let mut budget = SourceBudget {
        used: 0,
        exhausted: true,
    };
    let mut sources = Vec::new();
    let mut limitations = Vec::new();

    collect_target_roots(
        &root,
        vec![target],
        None,
        &mut budget,
        &mut sources,
        &mut limitations,
    )
    .unwrap();

    assert!(sources.is_empty());
    assert!(limitations.is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn target_collection_propagates_source_read_errors() {
    let root = temp_root();
    fs::create_dir_all(root.join("src/broken.rs")).unwrap();
    let target = TargetRoot {
        id: "demo".into(),
        import_name: "demo".into(),
        kind: "lib",
        source: root.join("src/broken.rs"),
        package_root: root.clone(),
        dependencies: Vec::new(),
        resolved_features: None,
    };
    let mut budget = SourceBudget::default();
    let mut sources = Vec::new();
    let mut limitations = Vec::new();

    assert!(collect_target_roots(
        &root,
        vec![target],
        None,
        &mut budget,
        &mut sources,
        &mut limitations,
    )
    .is_err());

    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn nested_module_read_errors_propagate_through_inline_and_file_discovery() {
    use std::os::unix::fs::PermissionsExt;

    let root = temp_root();
    fs::create_dir_all(root.join("src/outer")).unwrap();
    fs::write(root.join("src/lib.rs"), "mod outer { mod child; }").unwrap();
    let child = root.join("src/outer/child.rs");
    fs::write(&child, "pub fn child() {}").unwrap();
    let mut permissions = fs::metadata(&child).unwrap().permissions();
    permissions.set_mode(0o000);
    fs::set_permissions(&child, permissions).unwrap();

    let mut visited = BTreeSet::new();
    let mut sources = Vec::new();
    let mut limitations = Vec::new();
    let mut budget = SourceBudget::default();
    let result = collect_reachable_module(
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
    );

    let mut permissions = fs::metadata(&child).unwrap().permissions();
    permissions.set_mode(0o600);
    fs::set_permissions(&child, permissions).unwrap();
    assert!(result.is_err());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn module_cfg_discovery_covers_malformed_and_true_cfg_paths() {
    use crate::{cfg::HostCfg, model::AnalysisProfile, profile::ProfileContext};

    let root = temp_root();
    fs::write(
        root.join("src/lib.rs"),
        "#[cfg()] mod malformed; #[cfg(unix)] mod enabled;",
    )
    .unwrap();
    fs::write(root.join("src/enabled.rs"), "pub fn enabled() {}").unwrap();
    let profile = ProfileContext {
        public: AnalysisProfile {
            id: "test".into(),
            target: "host".into(),
            resolved_target: "test-target".into(),
            features: Vec::new(),
            target_cfg: Vec::new(),
        },
        cfg: HostCfg::test(&["unix"]),
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
        Some(&profile.cfg),
        &mut visited,
        &mut budget,
        &mut sources,
        &mut limitations,
    )
    .unwrap();

    assert!(sources
        .iter()
        .any(|source| source.relative_path == "src/enabled.rs"));
    assert!(limitations
        .iter()
        .any(|detail| detail.contains("unresolved cfg reachability")));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn fallback_inventory_propagates_missing_root_errors() {
    let root = temp_root();
    let missing = root.join("missing");
    assert!(super::fallback_inventory(&missing).is_err());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn fallback_collection_honors_preexhausted_and_newly_exhausted_budgets() {
    let root = temp_root();
    fs::write(root.join("src/lib.rs"), "pub fn demo() {}").unwrap();
    let target = root.join("target");

    let mut preexhausted = SourceBudget {
        used: 0,
        exhausted: true,
    };
    let mut sources = Vec::new();
    let mut limitations = Vec::new();
    collect_rust_files(
        &root,
        &root,
        &target,
        "demo",
        &mut preexhausted,
        &mut sources,
        &mut limitations,
    )
    .unwrap();
    assert!(sources.is_empty());

    let mut full = SourceBudget {
        used: MAX_SNAPSHOT_SOURCE_BYTES,
        exhausted: false,
    };
    collect_rust_files(
        &root,
        &root.join("src"),
        &target,
        "demo",
        &mut full,
        &mut sources,
        &mut limitations,
    )
    .unwrap();
    assert!(limitations
        .iter()
        .any(|detail| detail.contains("512 MiB snapshot limit")));

    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn fallback_recursive_directory_errors_are_propagated() {
    use std::os::unix::fs::PermissionsExt;

    let root = temp_root();
    let locked = root.join("locked");
    fs::create_dir_all(&locked).unwrap();
    let mut permissions = fs::metadata(&locked).unwrap().permissions();
    permissions.set_mode(0o000);
    fs::set_permissions(&locked, permissions).unwrap();

    let target = root.join("target");
    let mut budget = SourceBudget::default();
    let mut sources = Vec::new();
    let mut limitations = Vec::new();
    let result = collect_rust_files(
        &root,
        &root,
        &target,
        "demo",
        &mut budget,
        &mut sources,
        &mut limitations,
    );

    let mut permissions = fs::metadata(&locked).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&locked, permissions).unwrap();
    assert!(result.is_err());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn metadata_helpers_report_spawn_parse_manifest_and_io_errors() {
    let root = temp_root();
    let missing = root.join("missing");
    assert!(super::load_metadata(&missing, None)
        .unwrap_err()
        .contains("could not execute cargo metadata"));
    assert!(super::parse_metadata_output(b"{")
        .unwrap_err()
        .contains("invalid cargo metadata JSON"));
    assert!(super::manifest_parent("/").is_err());

    let error = std::io::Error::other("fixture");
    assert!(
        super::io_with_path::<()>(Err(error), "inspect", std::path::Path::new("fixture"))
            .unwrap_err()
            .contains("cannot inspect")
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn reachable_module_propagates_directory_read_errors() {
    let root = temp_root();
    let directory = root.join("src/directory.rs");
    fs::create_dir_all(&directory).unwrap();
    let mut visited = BTreeSet::new();
    let mut sources = Vec::new();
    let mut limitations = Vec::new();
    let mut budget = SourceBudget::default();

    let error = collect_reachable_module(
        &root,
        "demo",
        &directory,
        "directory",
        false,
        None,
        &mut visited,
        &mut budget,
        &mut sources,
        &mut limitations,
    )
    .unwrap_err();

    assert!(error.contains("cannot read"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn directory_entry_collection_maps_iterator_errors() {
    let root = temp_root();
    let entries = std::iter::once(Err::<fs::DirEntry, _>(std::io::Error::other(
        "fixture directory entry failure",
    )));

    let error = super::collect_directory_entries(entries, &root).unwrap_err();

    assert!(error.contains("read directory entry"));
    assert!(error.contains("fixture directory entry failure"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn inventory_and_acquisition_propagate_fallback_io_failures() {
    let root = temp_root();
    let not_directory = root.join("not-directory");
    fs::write(&not_directory, "file").unwrap();

    assert!(super::inventory(&not_directory).is_err());

    let bad_metadata = Metadata {
        packages: vec![Package {
            name: "demo".into(),
            id: "demo-id".into(),
            manifest_path: "/".into(),
            targets: Vec::new(),
            dependencies: Vec::new(),
        }],
        workspace_members: vec!["demo-id".into()],
        workspace_root: root.to_string_lossy().into_owned(),
        resolve: None,
    };
    assert!(super::acquire_inventory(&not_directory, Ok(bad_metadata), None).is_err());
    assert!(
        super::acquire_inventory(&not_directory, Err("metadata unavailable".into()), None,)
            .is_err()
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn metadata_inventory_rejects_manifest_without_parent() {
    let root = temp_root();
    let metadata = Metadata {
        packages: vec![Package {
            name: "demo".into(),
            id: "demo-id".into(),
            manifest_path: "/".into(),
            targets: Vec::new(),
            dependencies: Vec::new(),
        }],
        workspace_members: vec!["demo-id".into()],
        workspace_root: root.to_string_lossy().into_owned(),
        resolve: None,
    };

    assert!(inventory_from_metadata(&root, metadata, None)
        .unwrap_err()
        .contains("manifest has no parent"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn binary_only_package_has_no_implicit_library_alias() {
    let root = temp_root();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='demo'\nversion='0.1.0'\n",
    )
    .unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();

    let metadata = Metadata {
        packages: vec![Package {
            name: "demo".into(),
            id: "demo-id".into(),
            manifest_path: root.join("Cargo.toml").to_string_lossy().into_owned(),
            targets: vec![Target {
                name: "demo".into(),
                kind: vec!["bin".into()],
                src_path: root.join("src/main.rs").to_string_lossy().into_owned(),
            }],
            dependencies: Vec::new(),
        }],
        workspace_members: vec!["demo-id".into()],
        workspace_root: root.to_string_lossy().into_owned(),
        resolve: None,
    };

    let (sources, aliases, _, limitations) =
        inventory_from_metadata(&root, metadata, None).unwrap();

    assert_eq!(sources.len(), 1);
    assert!(aliases.get("demo").unwrap().is_empty());
    assert!(limitations.is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn fallback_directory_helpers_propagate_entry_and_metadata_errors() {
    let root = temp_root();
    let not_directory = root.join("file");
    fs::write(&not_directory, "x").unwrap();
    assert!(super::read_directory_paths(&not_directory).is_err());

    let entry_error = std::io::Error::other("entry failed");
    let entries: Vec<std::io::Result<fs::DirEntry>> = vec![Err(entry_error)];
    assert!(super::collect_directory_entries(entries, &root).is_err());

    let mut budget = SourceBudget::default();
    let mut sources = Vec::new();
    let mut limitations = Vec::new();
    assert!(super::collect_rust_paths(
        &root,
        vec![root.join("missing.rs")],
        &root.join("target"),
        "demo",
        &mut budget,
        &mut sources,
        &mut limitations,
    )
    .is_err());

    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn fallback_path_reader_propagates_regular_read_failures() {
    use std::os::unix::net::UnixListener;

    let root = temp_root();
    let socket = root.join("unreadable.rs");
    let listener = UnixListener::bind(&socket).unwrap();
    let mut budget = SourceBudget::default();
    let mut sources = Vec::new();
    let mut limitations = Vec::new();

    let error = super::collect_rust_paths(
        &root,
        vec![socket.clone()],
        &root.join("target"),
        "demo",
        &mut budget,
        &mut sources,
        &mut limitations,
    )
    .unwrap_err();

    assert!(error.contains("cannot read"));
    drop(listener);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn directory_path_reader_propagates_entry_iteration_failure() {
    let root = temp_root();
    let entries = std::iter::once(Err::<fs::DirEntry, _>(std::io::Error::other(
        "fixture entry iteration failure",
    )));

    let error =
        super::directory_paths_from_entries(super::collect_directory_entries(entries, &root))
            .unwrap_err();

    assert!(error.contains("fixture entry iteration failure"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn canonical_module_and_directory_path_helpers_propagate_supplied_errors() {
    let root = temp_root();
    let source = root.join("src/raced.rs");
    let mut visited = BTreeSet::new();
    let mut budget = SourceBudget::default();
    let mut sources = Vec::new();
    let mut limitations = Vec::new();

    let error = super::collect_canonical_module(
        &root,
        "demo",
        &source,
        "raced",
        false,
        None,
        1,
        Err(std::io::Error::other("canonicalization race")),
        &mut visited,
        &mut budget,
        &mut sources,
        &mut limitations,
    )
    .unwrap_err();
    assert!(error.contains("cannot resolve"));

    let error =
        super::directory_paths_from_entries(Err("directory entry race".into())).unwrap_err();
    assert_eq!(error, "directory entry race");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn auxiliary_target_summary_classifies_non_production_target_kinds() {
    let root = temp_root();
    let manifest = root.join("Cargo.toml");
    fs::write(&manifest, "[package]\nname='demo'\nversion='0.1.0'\n").unwrap();

    let metadata = Metadata {
        packages: vec![Package {
            name: "demo".into(),
            id: "demo-id".into(),
            manifest_path: manifest.to_string_lossy().into_owned(),
            targets: vec![
                Target {
                    name: "demo".into(),
                    kind: vec!["lib".into()],
                    src_path: root.join("src/lib.rs").to_string_lossy().into_owned(),
                },
                Target {
                    name: "integration".into(),
                    kind: vec!["test".into()],
                    src_path: root
                        .join("tests/integration.rs")
                        .to_string_lossy()
                        .into_owned(),
                },
                Target {
                    name: "bench".into(),
                    kind: vec!["bench".into()],
                    src_path: root.join("benches/bench.rs").to_string_lossy().into_owned(),
                },
                Target {
                    name: "example".into(),
                    kind: vec!["example".into()],
                    src_path: root
                        .join("examples/example.rs")
                        .to_string_lossy()
                        .into_owned(),
                },
                Target {
                    name: "build-script-build".into(),
                    kind: vec!["custom-build".into()],
                    src_path: root.join("build.rs").to_string_lossy().into_owned(),
                },
                Target {
                    name: "macro".into(),
                    kind: vec!["proc-macro".into()],
                    src_path: root.join("src/macro.rs").to_string_lossy().into_owned(),
                },
            ],
            dependencies: Vec::new(),
        }],
        workspace_members: vec!["demo-id".into()],
        workspace_root: root.to_string_lossy().into_owned(),
        resolve: None,
    };

    let summary = super::auxiliary_target_summary(&metadata);

    assert_eq!(summary.tests, 1);
    assert_eq!(summary.benches, 1);
    assert_eq!(summary.examples, 1);
    assert_eq!(summary.build_scripts, 1);
    assert_eq!(summary.proc_macros, 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cargo_resolution_identity_is_checkout_relative_and_tracks_lockfile_changes() {
    fn fixture(root: &std::path::Path) -> Metadata {
        let manifest = root.join("Cargo.toml");
        fs::write(
            &manifest,
            "[package]\nname='demo'\nversion='0.1.0'\nedition='2021'\n",
        )
        .unwrap();
        fs::write(root.join("Cargo.lock"), "version = 4\n").unwrap();
        fs::write(root.join("src/lib.rs"), "pub fn stable() {}\n").unwrap();
        Metadata {
            packages: vec![Package {
                name: "demo".into(),
                id: "demo 0.1.0 (path+file:///checkout)".into(),
                manifest_path: manifest.to_string_lossy().into_owned(),
                targets: vec![Target {
                    name: "demo".into(),
                    kind: vec!["lib".into()],
                    src_path: root.join("src/lib.rs").to_string_lossy().into_owned(),
                }],
                dependencies: Vec::new(),
            }],
            workspace_members: vec!["demo 0.1.0 (path+file:///checkout)".into()],
            workspace_root: root.to_string_lossy().into_owned(),
            resolve: None,
        }
    }

    let left = temp_root();
    let right = temp_root();
    let left_metadata = fixture(&left);
    let right_metadata = fixture(&right);

    let left_id = super::cargo_resolution_identity(&left, &left_metadata).unwrap();
    let right_id = super::cargo_resolution_identity(&right, &right_metadata).unwrap();
    assert_eq!(left_id, right_id);

    fs::write(right.join("Cargo.lock"), "version = 4\n# changed\n").unwrap();
    let changed = super::cargo_resolution_identity(&right, &right_metadata).unwrap();
    assert_ne!(left_id, changed);

    fs::remove_dir_all(left).unwrap();
    fs::remove_dir_all(right).unwrap();
}

#[test]
fn stability_verification_detects_source_and_cargo_input_changes() {
    let root = temp_root();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='demo'\nversion='0.1.0'\n",
    )
    .unwrap();
    fs::write(root.join("Cargo.lock"), "version = 4\n").unwrap();
    fs::write(root.join("src/lib.rs"), "pub fn stable() {}\n").unwrap();

    let source = super::SourceFile {
        crate_name: "demo".into(),
        module_path: String::new(),
        relative_path: "src/lib.rs".into(),
        bytes: fs::read(root.join("src/lib.rs")).unwrap(),
    };
    let cargo_digest = super::cargo_input_digest(&root).unwrap();

    super::verify_stable_inputs(&root, std::slice::from_ref(&source), &cargo_digest).unwrap();

    fs::write(root.join("src/lib.rs"), "pub fn changed() {}\n").unwrap();
    assert!(
        super::verify_stable_inputs(&root, std::slice::from_ref(&source), &cargo_digest)
            .unwrap_err()
            .contains("changed during analysis")
    );

    fs::write(root.join("src/lib.rs"), &source.bytes).unwrap();
    fs::write(root.join("Cargo.lock"), "version = 4\n# changed\n").unwrap();
    assert!(super::verify_stable_inputs(&root, &[source], &cargo_digest)
        .unwrap_err()
        .contains("Cargo inputs changed during analysis"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cargo_resolution_identity_handles_outside_and_missing_manifests_explicitly() {
    let root = temp_root();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='demo'\nversion='0.1.0'\n",
    )
    .unwrap();

    let outside = std::env::temp_dir().join(format!(
        "ferric-lens-outside-manifest-{}-{}",
        std::process::id(),
        TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&outside).unwrap();
    fs::write(
        outside.join("Cargo.toml"),
        "[package]\nname='outside'\nversion='0.1.0'\n",
    )
    .unwrap();

    let outside_metadata = Metadata {
        packages: vec![Package {
            name: "outside".into(),
            id: "outside-id".into(),
            manifest_path: outside.join("Cargo.toml").to_string_lossy().into_owned(),
            targets: Vec::new(),
            dependencies: Vec::new(),
        }],
        workspace_members: vec!["outside-id".into()],
        workspace_root: root.to_string_lossy().into_owned(),
        resolve: None,
    };
    assert!(super::cargo_resolution_identity(&root, &outside_metadata).is_ok());

    let missing_metadata = Metadata {
        packages: vec![Package {
            name: "missing".into(),
            id: "missing-id".into(),
            manifest_path: root
                .join("missing/Cargo.toml")
                .to_string_lossy()
                .into_owned(),
            targets: Vec::new(),
            dependencies: Vec::new(),
        }],
        workspace_members: vec!["missing-id".into()],
        workspace_root: root.to_string_lossy().into_owned(),
        resolve: None,
    };
    assert!(super::cargo_resolution_identity(&root, &missing_metadata)
        .unwrap_err()
        .contains("cannot read Cargo manifest"));

    fs::remove_dir_all(outside).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cargo_resolution_identity_normalizes_dependency_paths_and_ignores_outside_targets() {
    let root = temp_root();
    fs::create_dir_all(root.join("crates/shared")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='demo'\nversion='0.1.0'\n",
    )
    .unwrap();
    fs::write(root.join("src/lib.rs"), "pub fn stable() {}\n").unwrap();

    let outside_source = std::env::temp_dir().join(format!(
        "ferric-lens-outside-target-{}-{}.rs",
        std::process::id(),
        TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(&outside_source, "pub fn outside() {}\n").unwrap();

    let metadata = Metadata {
        packages: vec![Package {
            name: "demo".into(),
            id: "demo-id".into(),
            manifest_path: root.join("Cargo.toml").to_string_lossy().into_owned(),
            targets: vec![
                Target {
                    name: "demo".into(),
                    kind: vec!["lib".into()],
                    src_path: root.join("src/lib.rs").to_string_lossy().into_owned(),
                },
                Target {
                    name: "outside".into(),
                    kind: vec!["bin".into()],
                    src_path: outside_source.to_string_lossy().into_owned(),
                },
            ],
            dependencies: vec![
                Dependency {
                    name: "registry".into(),
                    rename: None,
                    path: None,
                },
                Dependency {
                    name: "shared".into(),
                    rename: Some("renamed_shared".into()),
                    path: Some(root.join("crates/shared").to_string_lossy().into_owned()),
                },
                Dependency {
                    name: "outside".into(),
                    rename: None,
                    path: Some(
                        std::env::temp_dir()
                            .join("ferric-lens-external-dependency")
                            .to_string_lossy()
                            .into_owned(),
                    ),
                },
            ],
        }],
        workspace_members: vec!["demo-id".into()],
        workspace_root: root.to_string_lossy().into_owned(),
        resolve: None,
    };

    let with_dependencies = super::cargo_resolution_identity(&root, &metadata).unwrap();

    let no_dependency_metadata = Metadata {
        packages: vec![Package {
            name: "demo".into(),
            id: "demo-id".into(),
            manifest_path: root.join("Cargo.toml").to_string_lossy().into_owned(),
            targets: vec![Target {
                name: "demo".into(),
                kind: vec!["lib".into()],
                src_path: root.join("src/lib.rs").to_string_lossy().into_owned(),
            }],
            dependencies: Vec::new(),
        }],
        workspace_members: vec!["demo-id".into()],
        workspace_root: root.to_string_lossy().into_owned(),
        resolve: None,
    };
    let without_dependencies =
        super::cargo_resolution_identity(&root, &no_dependency_metadata).unwrap();

    assert_ne!(with_dependencies, without_dependencies);

    fs::remove_file(outside_source).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn stability_verification_treats_a_disappearing_source_as_an_input_change() {
    let root = temp_root();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='demo'\nversion='0.1.0'\n",
    )
    .unwrap();
    fs::write(root.join("src/lib.rs"), "pub fn stable() {}\n").unwrap();

    let source = SourceFile {
        crate_name: "demo".into(),
        module_path: String::new(),
        relative_path: "src/lib.rs".into(),
        bytes: fs::read(root.join("src/lib.rs")).unwrap(),
    };
    let cargo_digest = super::cargo_input_digest(&root).unwrap();
    fs::remove_file(root.join("src/lib.rs")).unwrap();

    let error = super::verify_stable_inputs(&root, &[source], &cargo_digest).unwrap_err();
    assert!(error.contains("changed during analysis"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cargo_resolution_identity_covers_checkout_boundaries_dependencies_and_ordering() {
    let root = temp_root();
    let second_manifest = root.join("second/Cargo.toml");
    fs::create_dir_all(second_manifest.parent().unwrap()).unwrap();
    fs::create_dir_all(root.join("dep")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='demo'\nversion='0.1.0'\n",
    )
    .unwrap();
    fs::write(
        &second_manifest,
        "[package]\nname='second'\nversion='0.1.0'\n",
    )
    .unwrap();
    fs::write(root.join("Cargo.lock"), "version = 4\n").unwrap();
    fs::write(root.join("src/lib.rs"), "pub fn demo() {}\n").unwrap();

    let outside = std::env::temp_dir().join(format!(
        "ferric-lens-resolution-outside-{}-{}",
        std::process::id(),
        TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&outside).unwrap();
    fs::write(
        outside.join("Cargo.toml"),
        "[package]\nname='outside'\nversion='0.1.0'\n",
    )
    .unwrap();

    let first = || Package {
        name: "demo".into(),
        id: "demo-id".into(),
        manifest_path: root.join("Cargo.toml").to_string_lossy().into_owned(),
        targets: vec![
            Target {
                name: "demo".into(),
                kind: vec!["lib".into()],
                src_path: root.join("src/lib.rs").to_string_lossy().into_owned(),
            },
            Target {
                name: "outside".into(),
                kind: vec!["bin".into()],
                src_path: outside.join("main.rs").to_string_lossy().into_owned(),
            },
        ],
        dependencies: vec![
            super::Dependency {
                name: "inside".into(),
                rename: Some("renamed".into()),
                path: Some(root.join("dep").to_string_lossy().into_owned()),
            },
            super::Dependency {
                name: "outside".into(),
                rename: None,
                path: Some(outside.to_string_lossy().into_owned()),
            },
            super::Dependency {
                name: "registry".into(),
                rename: None,
                path: None,
            },
        ],
    };
    let second = || Package {
        name: "second".into(),
        id: "second-id".into(),
        manifest_path: second_manifest.to_string_lossy().into_owned(),
        targets: Vec::new(),
        dependencies: Vec::new(),
    };
    let outside_package = Package {
        name: "outside".into(),
        id: "outside-id".into(),
        manifest_path: outside.join("Cargo.toml").to_string_lossy().into_owned(),
        targets: Vec::new(),
        dependencies: Vec::new(),
    };
    let metadata = Metadata {
        packages: vec![second(), outside_package, first()],
        workspace_members: vec!["demo-id".into(), "second-id".into(), "outside-id".into()],
        workspace_root: root.to_string_lossy().into_owned(),
        resolve: None,
    };
    let first_id = super::cargo_resolution_identity(&root, &metadata).unwrap();

    let reordered = Metadata {
        packages: vec![first(), second()],
        workspace_members: vec!["second-id".into(), "demo-id".into()],
        workspace_root: root.to_string_lossy().into_owned(),
        resolve: None,
    };
    let reordered_id = super::cargo_resolution_identity(&root, &reordered).unwrap();

    assert_eq!(first_id, reordered_id);

    fs::remove_dir_all(outside).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cargo_resolution_identity_reports_missing_repository_manifest() {
    let root = temp_root();
    fs::write(root.join("Cargo.lock"), "version = 4\n").unwrap();
    let missing = root.join("missing/Cargo.toml");
    let metadata = Metadata {
        packages: vec![Package {
            name: "missing".into(),
            id: "missing-id".into(),
            manifest_path: missing.to_string_lossy().into_owned(),
            targets: Vec::new(),
            dependencies: Vec::new(),
        }],
        workspace_members: vec!["missing-id".into()],
        workspace_root: root.to_string_lossy().into_owned(),
        resolve: None,
    };

    let error = super::cargo_resolution_identity(&root, &metadata).unwrap_err();

    assert!(error.contains("cannot read Cargo manifest"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn stability_verification_reports_source_disappearance() {
    let root = temp_root();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='demo'\nversion='0.1.0'\n",
    )
    .unwrap();
    fs::write(root.join("src/lib.rs"), "pub fn stable() {}\n").unwrap();
    let source = super::SourceFile {
        crate_name: "demo".into(),
        module_path: String::new(),
        relative_path: "src/lib.rs".into(),
        bytes: fs::read(root.join("src/lib.rs")).unwrap(),
    };
    let cargo_digest = super::cargo_input_digest(&root).unwrap();
    fs::remove_file(root.join("src/lib.rs")).unwrap();

    let error = super::verify_stable_inputs(&root, &[source], &cargo_digest).unwrap_err();

    assert!(error.contains("changed during analysis"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cargo_resolution_identity_propagates_cargo_input_read_failure() {
    let root = temp_root();
    fs::create_dir(root.join("Cargo.toml")).unwrap();
    let metadata = Metadata {
        packages: Vec::new(),
        workspace_members: Vec::new(),
        workspace_root: root.to_string_lossy().into_owned(),
        resolve: None,
    };

    let error = super::cargo_resolution_identity(&root, &metadata).unwrap_err();

    assert!(error.contains("cannot read Cargo input"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn stability_verification_propagates_cargo_input_read_failure() {
    let root = temp_root();
    fs::create_dir(root.join("Cargo.toml")).unwrap();

    let error = super::verify_stable_inputs(&root, &[], "expected").unwrap_err();

    assert!(error.contains("cannot read Cargo input"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn inventory_assembly_propagates_resolution_and_fallback_errors() {
    let root = temp_root();
    fs::write(root.join("Cargo.lock"), "version = 4\n").unwrap();
    let cargo_inputs = super::cargo_input_snapshot(&root, None).unwrap();

    let missing_manifest = Metadata {
        packages: vec![Package {
            name: "missing".into(),
            id: "missing-id".into(),
            manifest_path: root
                .join("missing/Cargo.toml")
                .to_string_lossy()
                .into_owned(),
            targets: Vec::new(),
            dependencies: Vec::new(),
        }],
        workspace_members: vec!["missing-id".into()],
        workspace_root: root.to_string_lossy().into_owned(),
        resolve: None,
    };
    assert!(
        super::assemble_inventory(&root, Ok(missing_manifest), None, &cargo_inputs)
            .unwrap_err()
            .contains("cannot read Cargo manifest")
    );

    let not_directory = root.join("not-directory");
    fs::write(&not_directory, "not a directory").unwrap();
    assert!(super::assemble_inventory(
        &not_directory,
        Err("metadata unavailable".into()),
        None,
        &cargo_inputs,
    )
    .is_err());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn verified_inventory_propagates_source_stability_failure() {
    let root = temp_root();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='demo'\nversion='0.1.0'\n",
    )
    .unwrap();
    fs::write(root.join("src/lib.rs"), "pub fn current() {}\n").unwrap();
    let cargo_inputs = super::cargo_input_snapshot(&root, None).unwrap();
    let source = SourceFile {
        crate_name: "demo".into(),
        module_path: String::new(),
        relative_path: "src/lib.rs".into(),
        bytes: b"pub fn previous() {}\n".to_vec(),
    };

    let error = super::finalize_verified_inventory(
        &root,
        (
            vec![source],
            WorkspaceAliases::new(),
            BTreeMap::new(),
            true,
            None,
        ),
        &cargo_inputs,
        None,
        super::AuxiliaryTargetSummary::default(),
    )
    .unwrap_err();

    assert!(error.contains("changed during analysis"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn resolved_cargo_features_are_applied_per_workspace_package() {
    use crate::{cfg::HostCfg, model::AnalysisProfile, profile::ProfileContext};

    let root = temp_root();
    let a = root.join("crates/a");
    let b = root.join("crates/b");
    for package in [&a, &b] {
        fs::create_dir_all(package.join("src")).unwrap();
        fs::write(
            package.join("Cargo.toml"),
            "[package]\nname='fixture'\nversion='0.1.0'\n",
        )
        .unwrap();
        fs::write(
            package.join("src/lib.rs"),
            "#[cfg(feature = \"enabled\")] mod gated;\npub fn root() {}\n",
        )
        .unwrap();
        fs::write(package.join("src/gated.rs"), "pub fn gated() {}\n").unwrap();
    }

    let a_id = "a-id".to_owned();
    let b_id = "b-id".to_owned();
    let metadata = Metadata {
        packages: vec![
            Package {
                name: "a".into(),
                id: a_id.clone(),
                manifest_path: a.join("Cargo.toml").to_string_lossy().into_owned(),
                targets: vec![Target {
                    name: "a".into(),
                    kind: vec!["lib".into()],
                    src_path: a.join("src/lib.rs").to_string_lossy().into_owned(),
                }],
                dependencies: Vec::new(),
            },
            Package {
                name: "b".into(),
                id: b_id.clone(),
                manifest_path: b.join("Cargo.toml").to_string_lossy().into_owned(),
                targets: vec![Target {
                    name: "b".into(),
                    kind: vec!["lib".into()],
                    src_path: b.join("src/lib.rs").to_string_lossy().into_owned(),
                }],
                dependencies: Vec::new(),
            },
        ],
        workspace_members: vec![a_id.clone(), b_id.clone()],
        workspace_root: root.to_string_lossy().into_owned(),
        resolve: Some(Resolve {
            nodes: vec![
                ResolveNode {
                    id: a_id,
                    features: vec!["default".into(), "enabled".into()],
                },
                ResolveNode {
                    id: b_id,
                    features: vec!["default".into()],
                },
            ],
        }),
    };
    let profile = ProfileContext {
        public: AnalysisProfile {
            id: "fixture".into(),
            target: "host".into(),
            resolved_target: "x86_64-unknown-linux-gnu".into(),
            features: Vec::new(),
            target_cfg: Vec::new(),
        },
        cfg: HostCfg::test(&["unix", "target_os=\"linux\""]),
    };

    let (sources, _, resolved, limitations) =
        inventory_from_metadata(&root, metadata, Some(&profile)).unwrap();

    assert!(limitations.is_empty());
    assert_eq!(
        resolved.get("a").unwrap(),
        &vec!["default".to_owned(), "enabled".to_owned()]
    );
    assert_eq!(resolved.get("b").unwrap(), &vec!["default".to_owned()]);
    assert!(sources
        .iter()
        .any(|source| source.crate_name == "a" && source.module_path == "gated"));
    assert!(!sources
        .iter()
        .any(|source| source.crate_name == "b" && source.module_path == "gated"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn workspace_member_manifest_change_invalidates_captured_cargo_snapshot() {
    let root = temp_root();
    let member = root.join("crates/member");
    fs::create_dir_all(&member).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers=['crates/member']\n",
    )
    .unwrap();
    fs::write(root.join("Cargo.lock"), "version = 4\n").unwrap();
    fs::write(
        member.join("Cargo.toml"),
        "[package]\nname='member'\nversion='0.1.0'\n",
    )
    .unwrap();

    let member_id = "member-id".to_owned();
    let metadata = Metadata {
        packages: vec![Package {
            name: "member".into(),
            id: member_id.clone(),
            manifest_path: member.join("Cargo.toml").to_string_lossy().into_owned(),
            targets: Vec::new(),
            dependencies: Vec::new(),
        }],
        workspace_members: vec![member_id],
        workspace_root: root.to_string_lossy().into_owned(),
        resolve: None,
    };
    let snapshot = super::cargo_input_snapshot(&root, Some(&metadata)).unwrap();

    fs::write(
        member.join("Cargo.toml"),
        "[package]\nname='member'\nversion='0.2.0'\n",
    )
    .unwrap();

    let error = super::verify_cargo_inputs(&snapshot).unwrap_err();
    assert!(error.contains("crates/member/Cargo.toml changed during analysis"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cargo_input_label_falls_back_for_paths_outside_repository_root() {
    let root = temp_root();
    let outside = root.parent().unwrap().join("outside-Cargo.toml");

    assert_eq!(
        super::cargo_input_label(&root, &outside),
        outside.to_string_lossy()
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cargo_input_reread_errors_are_reported_explicitly() {
    let root = temp_root();
    let unreadable_as_file = root.join("Cargo.toml");
    fs::create_dir_all(&unreadable_as_file).unwrap();
    let snapshot = CargoInputSnapshot {
        entries: vec![CargoInputEntry {
            path: unreadable_as_file,
            label: "Cargo.toml".into(),
            bytes: None,
        }],
        digest: "fixture".into(),
    };

    let error = super::verify_cargo_inputs(&snapshot).unwrap_err();

    assert!(error.contains("cannot re-read Cargo input"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn stable_snapshot_reports_source_disappearance_before_publication() {
    let root = temp_root();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='x'\nversion='0.1.0'\n",
    )
    .unwrap();
    fs::write(root.join("Cargo.lock"), "version = 4\n").unwrap();
    let cargo = super::cargo_input_snapshot(&root, None).unwrap();
    let source = SourceFile {
        crate_name: "demo".into(),
        module_path: String::new(),
        relative_path: "src/missing.rs".into(),
        bytes: b"fn missing() {}".to_vec(),
    };

    let error = super::verify_stable_inputs_snapshot(&root, &[source], &cargo).unwrap_err();

    assert!(error.contains("repository source src/missing.rs changed during analysis"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn target_without_resolve_features_uses_unresolved_profile_cfg() {
    let root = temp_root();
    fs::write(
        root.join("src/lib.rs"),
        "#[cfg(feature = \"enabled\")]\nmod gated;\n",
    )
    .unwrap();
    fs::write(root.join("src/gated.rs"), "pub fn gated() {}\n").unwrap();

    let target = TargetRoot {
        id: "demo".into(),
        import_name: "demo".into(),
        kind: "lib",
        source: root.join("src/lib.rs"),
        package_root: root.clone(),
        dependencies: Vec::new(),
        resolved_features: None,
    };
    let profile = ProfileContext {
        public: AnalysisProfile {
            id: "fixture".into(),
            target: "host".into(),
            resolved_target: "x86_64-unknown-linux-gnu".into(),
            features: vec!["enabled".into()],
            target_cfg: Vec::new(),
        },
        cfg: HostCfg::test_with_features(&["unix", "target_os=\"linux\""], &["enabled"]),
    };
    let mut budget = SourceBudget::default();
    let mut sources = Vec::new();
    let mut limitations = Vec::new();

    collect_target_roots(
        &root,
        vec![target],
        Some(&profile),
        &mut budget,
        &mut sources,
        &mut limitations,
    )
    .unwrap();

    assert!(limitations.is_empty());
    assert!(sources.iter().any(|source| source.module_path == "gated"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn stable_snapshot_propagates_cargo_input_verification_errors() {
    let root = temp_root();
    let invalid = root.join("Cargo.toml");
    fs::create_dir_all(&invalid).unwrap();
    let snapshot = CargoInputSnapshot {
        entries: vec![CargoInputEntry {
            path: invalid,
            label: "Cargo.toml".into(),
            bytes: None,
        }],
        digest: "fixture".into(),
    };

    let error = super::verify_stable_inputs_snapshot(&root, &[], &snapshot).unwrap_err();

    assert!(error.contains("cannot re-read Cargo input"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn literal_path_modules_are_discovered_without_incompleteness() {
    let root = temp_root();
    fs::write(
        root.join("src/lib.rs"),
        "#[path = \"renamed_impl.rs\"] mod view;\n",
    )
    .unwrap();
    fs::write(root.join("src/renamed_impl.rs"), "pub fn render() {}\n").unwrap();

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

    assert!(limitations.is_empty());
    assert!(sources.iter().any(|source| {
        source.module_path == "view" && source.relative_path == "src/renamed_impl.rs"
    }));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn non_literal_path_modules_remain_explicitly_incomplete() {
    let root = temp_root();
    fs::write(root.join("src/lib.rs"), "#[path = SOME_PATH] mod view;\n").unwrap();

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

    assert!(limitations
        .iter()
        .any(|detail| detail.contains("path value is not a non-empty string literal")));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn literal_module_path_rejects_ambiguous_empty_and_list_forms() {
    fn module(source: &str) -> syn::ItemMod {
        let file = syn::parse_file(source).unwrap();
        match file.items.into_iter().next().unwrap() {
            syn::Item::Mod(module) => module,
            _ => panic!("fixture must parse as a module"),
        }
    }

    assert_eq!(
        super::literal_module_path(&module("mod plain;")).unwrap(),
        None
    );
    assert_eq!(
        super::literal_module_path(&module("#[path = \"custom.rs\"] mod custom;")).unwrap(),
        Some(std::path::PathBuf::from("custom.rs"))
    );
    assert_eq!(
        super::literal_module_path(&module("#[path = \"\"] mod empty;")).unwrap_err(),
        "path value is not a non-empty string literal"
    );
    assert_eq!(
        super::literal_module_path(&module("#[path(\"custom.rs\")] mod list;")).unwrap_err(),
        "path attribute is not name-value syntax"
    );
    assert_eq!(
        super::literal_module_path(&module(
            "#[path = \"one.rs\"] #[path = \"two.rs\"] mod duplicate;"
        ))
        .unwrap_err(),
        "multiple path attributes"
    );
}

#[test]
fn inline_path_module_remains_explicitly_incomplete() {
    let root = temp_root();
    fs::write(
        root.join("src/lib.rs"),
        "#[path = \"ignored.rs\"] mod inline { pub fn value() {} }\n",
    )
    .unwrap();

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

    assert!(limitations
        .iter()
        .any(|detail| detail.contains("inline module inline uses #[path]")));
    fs::remove_dir_all(root).unwrap();
}
