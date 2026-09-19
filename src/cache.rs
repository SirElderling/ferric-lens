use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Deserialize, Serialize};

use crate::{input::SourceFile, model::ModuleMetrics};

const RAW_FACT_SCHEMA: u32 = 3;
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

    pub fn load(&self, source: &SourceFile, profile_digest: &str) -> Option<ModuleMetrics> {
        let key = key(source, profile_digest);
        let path = self.directory.join(format!("{key}.json"));
        let bytes = fs::read(path).ok()?;
        let entry: RawFactEntry = serde_json::from_slice(&bytes).ok()?;

        if entry.schema != RAW_FACT_SCHEMA
            || entry.tool_version != env!("CARGO_PKG_VERSION")
            || entry.key != key
        {
            return None;
        }

        let payload = serde_json::to_vec(&entry.module)
            .expect("cached module schema contains only JSON-serializable values");
        if blake3::hash(&payload).to_hex().as_str() != entry.payload_digest {
            return None;
        }

        let mut module = entry.module;
        module.crate_name = source.crate_name.clone();
        module.module_path = source.module_path.clone();
        module.path = source.relative_path.clone();
        module.local_dependency_modules.clear();
        module.history = None;
        Some(module)
    }

    pub fn store(&self, source: &SourceFile, profile_digest: &str, module: &ModuleMetrics) {
        let _ = self.try_store(source, profile_digest, module);
    }

    fn try_store(
        &self,
        source: &SourceFile,
        profile_digest: &str,
        module: &ModuleMetrics,
    ) -> Result<(), String> {
        fs::create_dir_all(&self.directory)
            .map_err(|error| format!("cannot create fact cache: {error}"))?;

        let key = key(source, profile_digest);
        let mut raw = module.clone();
        raw.crate_name.clear();
        raw.module_path.clear();
        raw.path.clear();
        raw.local_dependency_modules.clear();
        raw.history = None;

        let payload = serde_json::to_vec(&raw)
            .expect("cached fact schema contains only JSON-serializable values");
        let entry = RawFactEntry {
            schema: RAW_FACT_SCHEMA,
            tool_version: env!("CARGO_PKG_VERSION").into(),
            key: key.clone(),
            payload_digest: blake3::hash(&payload).to_hex().to_string(),
            module: raw,
        };
        let bytes = serde_json::to_vec(&entry)
            .expect("cache entry schema contains only JSON-serializable values");
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
            .filter_map(cache_file)
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
            remove_cache_file(&path, len, &mut remaining);
        }
    }
}


fn cache_file(entry: fs::DirEntry) -> Option<(PathBuf, u64)> {
    let path = entry.path();
    cache_file_from_metadata(path, entry.metadata())
}

fn cache_file_from_metadata(
    path: PathBuf,
    metadata: std::io::Result<fs::Metadata>,
) -> Option<(PathBuf, u64)> {
    let metadata = metadata.ok()?;
    metadata.is_file().then_some((path, metadata.len()))
}

fn remove_cache_file(path: &Path, len: u64, remaining: &mut u64) {
    if fs::remove_file(path).is_ok() {
        *remaining = remaining.saturating_sub(len);
    }
}

fn key(source: &SourceFile, profile_digest: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"ferric-lens-raw-fact");
    hasher.update(&RAW_FACT_SCHEMA.to_le_bytes());
    hasher.update(env!("CARGO_PKG_VERSION").as_bytes());
    hasher.update(profile_digest.as_bytes());
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
#[path = "cache_tests.rs"]
mod tests;
