use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Deserialize, Serialize};

use crate::{input::SourceFile, model::ModuleMetrics};

const RAW_FACT_SCHEMA: u32 = 1;
const MAX_CACHE_BYTES: u64 = 256 * 1024 * 1024;
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Serialize, Deserialize)]
struct RawFactEntry {
    schema: u32,
    tool_version: String,
    key: String,
    payload_digest: String,
    module: ModuleMetrics,
}

pub struct RawFactCache {
    directory: PathBuf,
}

impl RawFactCache {
    pub fn new(repo_root: &Path) -> Self {
        Self {
            directory: repo_root
                .join(".ferric-lens")
                .join("cache")
                .join("v1")
                .join("raw"),
        }
    }

    pub fn load(&self, source: &SourceFile) -> Option<ModuleMetrics> {
        let key = key(source);
        let path = self.directory.join(format!("{key}.json"));
        let bytes = fs::read(path).ok()?;
        let entry: RawFactEntry = serde_json::from_slice(&bytes).ok()?;

        if entry.schema != RAW_FACT_SCHEMA
            || entry.tool_version != env!("CARGO_PKG_VERSION")
            || entry.key != key
        {
            return None;
        }

        let payload = serde_json::to_vec(&entry.module).ok()?;
        if blake3::hash(&payload).to_hex().as_str() != entry.payload_digest {
            return None;
        }

        let mut module = entry.module;
        module.crate_name = source.crate_name.clone();
        module.module_path = source.module_path.clone();
        module.path = source.relative_path.clone();
        module.local_dependency_modules.clear();
        Some(module)
    }

    pub fn store(&self, source: &SourceFile, module: &ModuleMetrics) {
        let _ = self.try_store(source, module);
    }

    fn try_store(&self, source: &SourceFile, module: &ModuleMetrics) -> Result<(), String> {
        fs::create_dir_all(&self.directory)
            .map_err(|error| format!("cannot create fact cache: {error}"))?;

        let key = key(source);
        let mut raw = module.clone();
        raw.crate_name.clear();
        raw.module_path.clear();
        raw.path.clear();
        raw.local_dependency_modules.clear();

        let payload = serde_json::to_vec(&raw)
            .map_err(|error| format!("cannot encode cached facts: {error}"))?;
        let entry = RawFactEntry {
            schema: RAW_FACT_SCHEMA,
            tool_version: env!("CARGO_PKG_VERSION").into(),
            key: key.clone(),
            payload_digest: blake3::hash(&payload).to_hex().to_string(),
            module: raw,
        };
        let bytes = serde_json::to_vec(&entry)
            .map_err(|error| format!("cannot encode fact cache entry: {error}"))?;
        let destination = self.directory.join(format!("{key}.json"));
        atomic_write(&destination, &bytes)?;
        self.evict_if_needed();
        Ok(())
    }

    fn evict_if_needed(&self) {
        let Ok(entries) = fs::read_dir(&self.directory) else {
            return;
        };

        let mut files = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let path = entry.path();
                let metadata = entry.metadata().ok()?;
                metadata.is_file().then_some((path, metadata.len()))
            })
            .collect::<Vec<_>>();

        let total = files.iter().map(|(_, len)| *len).sum::<u64>();
        if total <= MAX_CACHE_BYTES {
            return;
        }

        files.sort_by(|a, b| a.0.file_name().cmp(&b.0.file_name()));
        let mut remaining = total;
        for (path, len) in files.into_iter().rev() {
            if remaining <= MAX_CACHE_BYTES {
                break;
            }
            if fs::remove_file(path).is_ok() {
                remaining = remaining.saturating_sub(len);
            }
        }
    }
}

fn key(source: &SourceFile) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"ferric-lens-raw-fact");
    hasher.update(&RAW_FACT_SCHEMA.to_le_bytes());
    hasher.update(env!("CARGO_PKG_VERSION").as_bytes());
    hasher.update(&source.bytes);
    hasher.finalize().to_hex().to_string()
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("cache path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;

    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(".raw.tmp-{}-{counter}", std::process::id()));
    fs::write(&temporary, bytes)
        .map_err(|error| format!("cannot write cache temporary file: {error}"))?;

    match fs::rename(&temporary, path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            Err(format!("cannot replace cache entry: {error}"))
        }
    }
}

#[cfg(test)]
mod tests {
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
        }
    }

    #[test]
    fn reuses_content_facts_across_movement_without_stale_identity() {
        let root = temp_root();
        let cache = RawFactCache::new(&root);
        let original = source("src/engine.rs");
        cache.store(&original, &module("src/engine.rs"));

        let moved = SourceFile {
            crate_name: "demo".into(),
            module_path: "nested::engine".into(),
            relative_path: "src/nested/engine.rs".into(),
            bytes: original.bytes.clone(),
        };
        let cached = cache.load(&moved).unwrap();

        assert_eq!(cached.module_path, "nested::engine");
        assert_eq!(cached.path, "src/nested/engine.rs");
        assert!(cached.local_dependency_modules.is_empty());
        assert_eq!(cached.explicit_imports.len(), 1);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrupt_entry_is_a_cache_miss() {
        let root = temp_root();
        let cache = RawFactCache::new(&root);
        let source = source("src/engine.rs");
        cache.store(&source, &module("src/engine.rs"));

        let cache_dir = root.join(".ferric-lens/cache/v1/raw");
        let entry = fs::read_dir(&cache_dir).unwrap().next().unwrap().unwrap();
        fs::write(entry.path(), b"not-json").unwrap();

        assert!(cache.load(&source).is_none());

        fs::remove_dir_all(root).unwrap();
    }
}
