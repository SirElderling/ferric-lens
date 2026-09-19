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
mod tests {
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    use crate::{
        model::{DeltaStatus, Evidence, EvidenceClass, Finding, Priority},
        profile::ProfileContext,
    };

    use super::{apply, fingerprint_findings, load, record};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_root() -> PathBuf {
        let counter = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "ferric-lens-acceptance-test-{}-{counter}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/lib.rs"), "pub fn stable() {}").unwrap();
        root
    }

    fn finding(value: usize) -> Finding {
        Finding {
            fingerprint: String::new(),
            rule: "structure.coupled_complexity_growth".into(),
            subject: "demo::engine".into(),
            identity: "demo::engine".into(),
            configuration: "target=host;features=default".into(),
            evidence_class: EvidenceClass::Strong,
            priority: Priority::ActFirst,
            delta: DeltaStatus::Worsened,
            gate: true,
            accepted: false,
            acceptance_reason: None,
            summary: "summary".into(),
            direction: "direction".into(),
            evidence: vec![Evidence {
                metric: "decision_sites".into(),
                value,
                reference: 15,
                population: 20,
                baseline: Some(12),
                material_delta: Some(3),
            }],
        }
    }

    #[test]
    fn presentation_does_not_change_fingerprint() {
        let mut left = finding(18);
        let mut right = finding(18);
        right.summary = "different presentation".into();
        right.subject = "moved::display".into();

        fingerprint_findings(std::slice::from_mut(&mut left));
        fingerprint_findings(std::slice::from_mut(&mut right));

        assert_eq!(left.fingerprint, right.fingerprint);
    }

    #[test]
    fn material_evidence_change_invalidates_fingerprint() {
        let mut left = finding(18);
        let mut right = finding(19);

        fingerprint_findings(std::slice::from_mut(&mut left));
        fingerprint_findings(std::slice::from_mut(&mut right));

        assert_ne!(left.fingerprint, right.fingerprint);
    }

    #[test]
    fn missing_acceptance_file_is_empty_and_apply_resets_state() {
        let root = temp_root();
        let set = load(&root).unwrap();
        let mut value = finding(18);
        value.fingerprint = "missing".into();
        value.accepted = true;
        value.acceptance_reason = Some("stale".into());

        apply(std::slice::from_mut(&mut value), &set);

        assert!(!value.accepted);
        assert!(value.acceptance_reason.is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn record_round_trips_trimmed_acceptance_and_applies_it() {
        let root = temp_root();
        let profile = ProfileContext::resolve(None, &[]).unwrap();
        let digest = crate::input::inventory_with_profile(&root, &profile)
            .unwrap()
            .content_digest;

        record(
            &root,
            &profile,
            "  fingerprint  ",
            "  accepted reason  ",
            &digest,
        )
        .unwrap();

        let set = load(&root).unwrap();
        let mut value = finding(18);
        value.fingerprint = "fingerprint".into();
        apply(std::slice::from_mut(&mut value), &set);

        assert!(value.accepted);
        assert_eq!(value.acceptance_reason.as_deref(), Some("accepted reason"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn record_rejects_empty_fields_and_changed_source_digest() {
        let root = temp_root();
        let profile = ProfileContext::resolve(None, &[]).unwrap();
        let digest = crate::input::inventory_with_profile(&root, &profile)
            .unwrap()
            .content_digest;

        assert!(record(&root, &profile, " ", "reason", &digest)
            .unwrap_err()
            .contains("fingerprint"));
        assert!(record(&root, &profile, "fingerprint", " ", &digest)
            .unwrap_err()
            .contains("reason"));
        assert!(record(&root, &profile, "fingerprint", "reason", "wrong")
            .unwrap_err()
            .contains("source changed"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn load_rejects_invalid_utf8_toml_version_empty_and_duplicate_entries() {
        let root = temp_root();
        let directory = root.join(".ferric-lens");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("acceptances.toml");

        fs::write(&path, [0xff]).unwrap();
        assert!(load(&root).unwrap_err().contains("not UTF-8"));

        fs::write(&path, "not = [valid").unwrap();
        assert!(load(&root).unwrap_err().contains("invalid acceptance file"));

        fs::write(&path, "version = 2\nacceptances = []\n").unwrap();
        assert!(load(&root)
            .unwrap_err()
            .contains("unsupported acceptance file version"));

        fs::write(
            &path,
            "version = 1\n[[acceptances]]\nfingerprint = \"\"\nreason = \"r\"\n",
        )
        .unwrap();
        assert!(load(&root).unwrap_err().contains("must be non-empty"));

        fs::write(
            &path,
            "version = 1\n[[acceptances]]\nfingerprint = \"same\"\nreason = \"one\"\n[[acceptances]]\nfingerprint = \"same\"\nreason = \"two\"\n",
        )
        .unwrap();
        assert!(load(&root).unwrap_err().contains("duplicate acceptance"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn load_reports_non_file_read_errors() {
        let root = temp_root();
        let path = root.join(".ferric-lens/acceptances.toml");
        fs::create_dir_all(&path).unwrap();

        assert!(load(&root)
            .unwrap_err()
            .contains("cannot read acceptance file"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fingerprint_covers_optional_evidence_and_unknown_rules_deterministically() {
        let mut value = finding(18);
        value.rule = "future.rule".into();
        value.evidence[0].baseline = None;
        value.evidence[0].material_delta = None;

        fingerprint_findings(std::slice::from_mut(&mut value));

        assert_eq!(value.fingerprint.len(), 64);
    }

    #[test]
    fn atomic_write_reports_invalid_parent_and_replace_errors() {
        assert!(super::atomic_write(std::path::Path::new(""), b"x")
            .unwrap_err()
            .contains("has no parent"));

        let root = temp_root();
        let destination = root.join("target");
        fs::create_dir_all(&destination).unwrap();
        assert!(super::atomic_write(&destination, b"x")
            .unwrap_err()
            .contains("cannot replace"));

        fs::remove_dir_all(root).unwrap();
    }
}
