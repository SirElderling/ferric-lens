use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{model::Finding, profile::ProfileContext};

const ACCEPTANCE_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Acceptance {
    pub fingerprint: String,
    pub reason: String,
}

#[derive(Debug, Default)]
pub struct AcceptanceSet {
    by_fingerprint: BTreeMap<String, String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct AcceptanceFile {
    version: u32,
    #[serde(default)]
    acceptances: Vec<Acceptance>,
}

pub fn load(root: &Path) -> Result<AcceptanceSet, String> {
    let path = acceptance_path(root);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(AcceptanceSet::default());
        }
        Err(error) => {
            return Err(format!(
                "cannot read acceptance file {}: {error}",
                path.display()
            ));
        }
    };

    let text = std::str::from_utf8(&bytes)
        .map_err(|error| format!("acceptance file is not UTF-8: {error}"))?;
    let file: AcceptanceFile =
        toml::from_str(text).map_err(|error| format!("invalid acceptance file: {error}"))?;

    if file.version != ACCEPTANCE_VERSION {
        return Err(format!(
            "unsupported acceptance file version {}; expected {}",
            file.version, ACCEPTANCE_VERSION
        ));
    }

    let mut by_fingerprint = BTreeMap::new();
    for acceptance in file.acceptances {
        let fingerprint = acceptance.fingerprint.trim();
        let reason = acceptance.reason.trim();
        if fingerprint.is_empty() || reason.is_empty() {
            return Err("acceptance fingerprint and reason must be non-empty".into());
        }
        if by_fingerprint
            .insert(fingerprint.to_owned(), reason.to_owned())
            .is_some()
        {
            return Err(format!("duplicate acceptance fingerprint {fingerprint}"));
        }
    }

    Ok(AcceptanceSet { by_fingerprint })
}

pub fn fingerprint_findings(findings: &mut [Finding]) {
    for finding in findings {
        finding.fingerprint = fingerprint(finding);
    }
}

pub fn apply(findings: &mut [Finding], acceptances: &AcceptanceSet) {
    for finding in findings {
        finding.accepted = false;
        finding.acceptance_reason = None;
        if let Some(reason) = acceptances.by_fingerprint.get(&finding.fingerprint) {
            finding.accepted = true;
            finding.acceptance_reason = Some(reason.clone());
        }
    }
}

pub fn record(
    root: &Path,
    profile: &ProfileContext,
    fingerprint: &str,
    reason: &str,
    expected_source_digest: &str,
) -> Result<(), String> {
    let fingerprint = fingerprint.trim();
    let reason = reason.trim();
    if fingerprint.is_empty() {
        return Err("finding fingerprint must be non-empty".into());
    }
    if reason.is_empty() {
        return Err("acceptance reason must be non-empty".into());
    }

    let actual_digest = crate::input::inventory_with_profile(root, profile)?.content_digest;
    if actual_digest != expected_source_digest {
        return Err(
            "repository source changed after analysis; rerun accept against the current result"
                .into(),
        );
    }

    let existing = load(root)?;
    let mut by_fingerprint = existing.by_fingerprint;
    by_fingerprint.insert(fingerprint.to_owned(), reason.to_owned());

    let file = AcceptanceFile {
        version: ACCEPTANCE_VERSION,
        acceptances: by_fingerprint
            .into_iter()
            .map(|(fingerprint, reason)| Acceptance {
                fingerprint,
                reason,
            })
            .collect(),
    };

    let text = toml::to_string_pretty(&file)
        .map_err(|error| format!("cannot serialize acceptances: {error}"))?;
    atomic_write(&acceptance_path(root), text.as_bytes())
}

fn fingerprint(finding: &Finding) -> String {
    let mut hasher = blake3::Hasher::new();
    feed(&mut hasher, "ferric-lens-finding");
    feed(&mut hasher, &finding.configuration);
    feed(&mut hasher, &finding.rule);
    feed(&mut hasher, rule_revision(&finding.rule));
    feed(&mut hasher, &finding.identity);

    for evidence in &finding.evidence {
        feed(&mut hasher, &evidence.metric);
        feed_usize(&mut hasher, evidence.value);
        feed_usize(&mut hasher, evidence.reference);
        feed_usize(&mut hasher, evidence.population);
        feed_optional_usize(&mut hasher, evidence.baseline);
        feed_optional_usize(&mut hasher, evidence.material_delta);
    }

    hasher.finalize().to_hex().to_string()
}

fn rule_revision(rule: &str) -> &'static str {
    match rule {
        "structure.current_coupled_outlier" => "1",
        "structure.coupled_complexity_growth" => "1",
        _ => "1",
    }
}

fn feed(hasher: &mut blake3::Hasher, value: &str) {
    hasher.update(&(value.len() as u64).to_le_bytes());
    hasher.update(value.as_bytes());
}

fn feed_usize(hasher: &mut blake3::Hasher, value: usize) {
    hasher.update(&(value as u64).to_le_bytes());
}

fn feed_optional_usize(hasher: &mut blake3::Hasher, value: Option<usize>) {
    match value {
        Some(value) => {
            hasher.update(&[1]);
            feed_usize(hasher, value);
        }
        None => {
            hasher.update(&[0]);
        }
    }
}

fn acceptance_path(root: &Path) -> PathBuf {
    root.join(".ferric-lens").join("acceptances.toml")
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("acceptance path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;

    let temporary = parent.join(format!(".acceptances.toml.tmp-{}", std::process::id()));
    fs::write(&temporary, bytes)
        .map_err(|error| format!("cannot write {}: {error}", temporary.display()))?;
    fs::rename(&temporary, path)
        .map_err(|error| format!("cannot replace {}: {error}", path.display()))
}

#[cfg(test)]
#[path = "acceptance_tests.rs"]
mod tests;
