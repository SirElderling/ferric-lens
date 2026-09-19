use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use crate::{
    input::SourceFile,
    model::{ImportPath, ModuleMetrics},
};

use super::RawFactCache;

static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_root() -> PathBuf {
    let counter = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "ferric-lens-cache-test-{}-{counter}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}

fn source(path: &str) -> SourceFile {
    SourceFile {
        crate_name: "demo".into(),
        module_path: "engine".into(),
        relative_path: path.into(),
        bytes: b"use crate::model::Thing; fn run() {}".to_vec(),
    }
}

fn module(path: &str) -> ModuleMetrics {
    ModuleMetrics {
        crate_name: "demo".into(),
        module_path: "engine".into(),
        path: path.into(),
        lines: 1,
        decision_sites: 0,
        public_items: 0,
        explicit_imports: vec![ImportPath {
            segments: vec!["crate".into(), "model".into(), "Thing".into()],
            glob: false,
        }],
        local_dependency_modules: vec!["model".into()],
        structure_digest: "digest".into(),
        parse_complete: true,
        gate_complete: true,
        limitation: None,
        history: None,
    }
}

#[test]
fn reuses_content_facts_across_movement_without_stale_identity() {
    let root = temp_root();
    let cache = RawFactCache::new(&root);
    let original = source("src/engine.rs");
    cache.store(&original, "profile", &module("src/engine.rs"));

    let moved = SourceFile {
        crate_name: "demo".into(),
        module_path: "nested::engine".into(),
        relative_path: "src/nested/engine.rs".into(),
        bytes: original.bytes.clone(),
    };
    let cached = cache.load(&moved, "profile").unwrap();

    assert_eq!(cached.module_path, "nested::engine");
    assert_eq!(cached.path, "src/nested/engine.rs");
    assert!(cached.local_dependency_modules.is_empty());
    assert_eq!(cached.explicit_imports.len(), 1);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn profile_digest_separates_platform_specific_facts() {
    let root = temp_root();
    let cache = RawFactCache::new(&root);
    let source = source("src/engine.rs");
    cache.store(&source, "linux", &module("src/engine.rs"));

    assert!(cache.load(&source, "macos").is_none());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn profile_digest_prevents_cross_profile_reuse() {
    let root = temp_root();
    let cache = RawFactCache::new(&root);
    let source = source("src/engine.rs");
    cache.store(&source, "linux", &module("src/engine.rs"));

    assert!(cache.load(&source, "macos").is_none());
    assert!(cache.load(&source, "linux").is_some());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn corrupt_entry_is_a_cache_miss() {
    let root = temp_root();
    let cache = RawFactCache::new(&root);
    let source = source("src/engine.rs");
    cache.store(&source, "profile", &module("src/engine.rs"));

    let cache_dir = root.join(".ferric-lens/cache/v1/raw");
    let entry = fs::read_dir(&cache_dir).unwrap().next().unwrap().unwrap();
    fs::write(entry.path(), b"not-json").unwrap();

    assert!(cache.load(&source, "profile").is_none());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn missing_and_tampered_entries_are_cache_misses() {
    let root = temp_root();
    let cache = RawFactCache::new(&root);
    let source = source("src/engine.rs");
    assert!(cache.load(&source, "profile").is_none());

    for field in ["schema", "tool_version", "key", "payload_digest"] {
        cache.store(&source, "profile", &module("src/engine.rs"));
        let cache_dir = root.join(".ferric-lens/cache/v1/raw");
        let entry = fs::read_dir(&cache_dir).unwrap().next().unwrap().unwrap();
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(entry.path()).unwrap()).unwrap();
        match field {
            "schema" => value["schema"] = serde_json::json!(999),
            "tool_version" => value["tool_version"] = serde_json::json!("other"),
            "key" => value["key"] = serde_json::json!("other"),
            "payload_digest" => value["payload_digest"] = serde_json::json!("other"),
            _ => unreachable!(),
        }
        fs::write(entry.path(), serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(cache.load(&source, "profile").is_none(), "{field}");
        fs::remove_dir_all(&cache_dir).unwrap();
    }

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn store_failure_is_non_fatal_but_try_store_reports_it() {
    let root = temp_root();
    fs::write(root.join(".ferric-lens"), "not a directory").unwrap();
    let cache = RawFactCache::new(&root);
    let source = source("src/engine.rs");
    let module = module("src/engine.rs");

    cache.store(&source, "profile", &module);
    assert!(cache.try_store(&source, "profile", &module).is_err());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn eviction_is_bounded_and_deterministic_by_filename() {
    let root = temp_root();
    let cache = RawFactCache::new(&root);
    fs::create_dir_all(&cache.directory).unwrap();

    for name in ["a.json", "b.json", "c.json"] {
        let file = fs::File::create(cache.directory.join(name)).unwrap();
        file.set_len(100 * 1024 * 1024).unwrap();
    }

    cache.evict_if_needed();

    assert!(cache.directory.join("a.json").exists());
    assert!(cache.directory.join("b.json").exists());
    assert!(!cache.directory.join("c.json").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn eviction_without_a_directory_is_a_noop() {
    let root = temp_root();
    let cache = RawFactCache::new(&root);

    cache.evict_if_needed();

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn atomic_write_reports_invalid_parent_and_cleans_failed_replacement() {
    assert!(super::atomic_write(std::path::Path::new(""), b"x")
        .unwrap_err()
        .contains("has no parent"));

    let root = temp_root();
    let destination = root.join("entry.json");
    fs::create_dir_all(&destination).unwrap();
    assert!(super::atomic_write(&destination, b"x")
        .unwrap_err()
        .contains("cannot replace"));
    assert!(fs::read_dir(&root).unwrap().all(|entry| !entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".raw.tmp-")));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn key_changes_with_content_profile_and_schema_inputs() {
    let first = source("src/engine.rs");
    let mut second = source("src/other.rs");
    second.bytes.push(b' ');

    assert_eq!(
        super::key(&first, "profile"),
        super::key(&source("moved.rs"), "profile")
    );
    assert_ne!(
        super::key(&first, "profile"),
        super::key(&second, "profile")
    );
    assert_ne!(super::key(&first, "profile"), super::key(&first, "other"));
}
