use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use super::{
    acquire_inventory, collect_reachable_module, collect_rust_files, collect_target_roots,
    finalize_inventory, inventory_from_metadata, module_path_from_relative, rust_name, Dependency,
    Metadata, Package, SourceBudget, SourceFile, Target, TargetRoot, WorkspaceAliases,
    MAX_SNAPSHOT_SOURCE_BYTES,
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

    let (sources, aliases, limitations) = inventory_from_metadata(&root, metadata, None).unwrap();

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

    let (sources, aliases, limitations) = inventory_from_metadata(&root, metadata, None).unwrap();

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
fn metadata_inventory_failure_falls_back_without_losing_source_visibility() {
    let root = temp_root();
    fs::write(root.join("src/fallback.rs"), "pub fn fallback() {}").unwrap();
    let broken_target = root.join("src/broken.rs");
    fs::create_dir_all(&broken_target).unwrap();

    let package_id = "broken-id".to_owned();
    let metadata = Metadata {
        packages: vec![Package {
            name: "broken".into(),
            id: package_id.clone(),
            manifest_path: root.join("Cargo.toml").to_string_lossy().into_owned(),
            targets: vec![Target {
                name: "broken".into(),
                kind: vec!["lib".into()],
                src_path: broken_target.to_string_lossy().into_owned(),
            }],
            dependencies: Vec::new(),
        }],
        workspace_members: vec![package_id],
        workspace_root: root.to_string_lossy().into_owned(),
    };

    let (sources, aliases, complete, detail) =
        super::inventory_from_metadata_result(&root, None, Ok(metadata)).unwrap();

    assert!(!complete);
    assert!(aliases.is_empty());
    assert!(sources
        .iter()
        .any(|source| source.relative_path == "src/fallback.rs"));
    assert!(detail
        .as_deref()
        .unwrap()
        .contains("Cargo metadata inventory failed"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn final_inventory_deduplicates_source_identity_and_hashes_workspace_aliases() {
    let root = temp_root();
    let source = super::SourceFile {
        crate_name: "demo".into(),
        module_path: "engine".into(),
        relative_path: "src/engine.rs".into(),
        bytes: b"pub fn run() {}".to_vec(),
    };
    let aliases = super::WorkspaceAliases::from([(
        "demo".into(),
        std::collections::BTreeMap::from([("shared".into(), "shared_crate".into())]),
    )]);

    let first =
        super::finalize_inventory(vec![source.clone(), source], aliases.clone(), true, None);
    let second = super::finalize_inventory(
        first.sources.clone(),
        super::WorkspaceAliases::new(),
        true,
        None,
    );

    assert_eq!(first.sources.len(), 1);
    assert_eq!(first.workspace_aliases, aliases);
    assert_ne!(first.content_digest, second.content_digest);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn metadata_inventory_skips_outside_packages_invalid_targets_and_non_workspace_dependencies() {
    let root = temp_root();
    let inside = root.join("crates/inside");
    fs::create_dir_all(inside.join("src")).unwrap();
    fs::write(
        inside.join("Cargo.toml"),
        "[package]\nname='inside'\nversion='0.1.0'\n",
    )
    .unwrap();
    fs::write(inside.join("src/lib.rs"), "pub fn inside() {}").unwrap();
    fs::write(inside.join("src/not-rust.txt"), "not rust").unwrap();

    let outside = std::env::temp_dir().join(format!(
        "ferric-lens-outside-package-{}-{}",
        std::process::id(),
        TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(outside.join("src")).unwrap();
    fs::write(
        outside.join("Cargo.toml"),
        "[package]\nname='outside'\nversion='0.1.0'\n",
    )
    .unwrap();
    fs::write(outside.join("src/lib.rs"), "pub fn outside() {}").unwrap();

    let inside_id = "inside-id".to_owned();
    let outside_id = "outside-id".to_owned();
    let metadata = Metadata {
        packages: vec![
            Package {
                name: "inside".into(),
                id: inside_id.clone(),
                manifest_path: inside.join("Cargo.toml").to_string_lossy().into_owned(),
                targets: vec![
                    Target {
                        name: "inside".into(),
                        kind: vec!["lib".into()],
                        src_path: inside.join("src/lib.rs").to_string_lossy().into_owned(),
                    },
                    Target {
                        name: "invalid".into(),
                        kind: vec!["bin".into()],
                        src_path: inside
                            .join("src/not-rust.txt")
                            .to_string_lossy()
                            .into_owned(),
                    },
                ],
                dependencies: vec![
                    super::Dependency {
                        name: "registry".into(),
                        rename: None,
                        path: None,
                    },
                    super::Dependency {
                        name: "missing-local".into(),
                        rename: None,
                        path: Some(root.join("crates/missing").to_string_lossy().into_owned()),
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
        workspace_members: vec![inside_id, outside_id],
        workspace_root: root.to_string_lossy().into_owned(),
    };

    let (sources, aliases, limitations) = inventory_from_metadata(&root, metadata, None).unwrap();

    assert!(limitations.is_empty());
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].crate_name, "inside");
    assert!(aliases.get("inside").unwrap().is_empty());

    fs::remove_dir_all(outside).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cfg_state_covers_malformed_and_known_true_attributes() {
    use crate::{cfg::HostCfg, model::AnalysisProfile, profile::ProfileContext};

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

    let malformed = syn::parse_file("#[cfg()] mod child;").unwrap();
    let syn::Item::Mod(malformed_module) = &malformed.items[0] else {
        panic!("expected module");
    };
    assert_eq!(
        super::module_cfg_state(&malformed_module.attrs, Some(&profile)),
        crate::cfg::Truth::Unknown
    );

    let enabled = syn::parse_file("#[cfg(unix)] mod child;").unwrap();
    let syn::Item::Mod(enabled_module) = &enabled.items[0] else {
        panic!("expected module");
    };
    assert_eq!(
        super::module_cfg_state(&enabled_module.attrs, Some(&profile)),
        crate::cfg::Truth::True
    );
}

#[test]
fn fallback_collection_honors_preexhausted_and_newly_exhausted_budgets() {
    let root = temp_root();
    fs::write(root.join("src/one.rs"), "pub fn one() {}").unwrap();
    let target = root.join("target");

    let mut already_exhausted = SourceBudget {
        used: 0,
        exhausted: true,
    };
    let mut sources = Vec::new();
    let mut limitations = Vec::new();
    super::collect_rust_files(
        &root,
        &root,
        &target,
        "demo",
        &mut already_exhausted,
        &mut sources,
        &mut limitations,
    )
    .unwrap();
    assert!(sources.is_empty());

    let mut exhausted_on_file = SourceBudget {
        used: super::MAX_SNAPSHOT_SOURCE_BYTES,
        exhausted: false,
    };
    super::collect_rust_files(
        &root,
        &root,
        &target,
        "demo",
        &mut exhausted_on_file,
        &mut sources,
        &mut limitations,
    )
    .unwrap();
    assert!(limitations
        .iter()
        .any(|detail| detail.contains("fallback source input exceeds")));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn fallback_inventory_reports_unreadable_root_shape() {
    let root = temp_root();
    let file = root.join("not-a-directory");
    fs::write(&file, "plain file").unwrap();

    let error = super::fallback_inventory(&file).unwrap_err();

    assert!(error.contains("cannot read"));
    fs::remove_dir_all(root).unwrap();
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
    };

    let (_, aliases, complete, detail) =
        acquire_inventory(&root, Ok(metadata), None).unwrap();

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
        true,
        None,
    ));
    let without_aliases = finalize_inventory((
        inventory.sources.clone(),
        WorkspaceAliases::new(),
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
    };

    let (sources, aliases, limitations) =
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
    use crate::{
        cfg::HostCfg,
        model::AnalysisProfile,
        profile::ProfileContext,
    };

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
        Some(&profile),
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
