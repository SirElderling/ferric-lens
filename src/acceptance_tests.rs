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

#[test]
fn atomic_write_reports_parent_creation_failure() {
    let root = temp_root();
    let blocker = root.join("blocker");
    fs::write(&blocker, "file").unwrap();

    let error = super::atomic_write(&blocker.join("acceptances.toml"), b"x").unwrap_err();

    assert!(error.contains("cannot create"));
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn atomic_write_reports_temporary_write_failure() {
    use std::os::unix::fs::PermissionsExt;

    let root = temp_root();
    let locked = root.join("locked");
    fs::create_dir_all(&locked).unwrap();
    let mut permissions = fs::metadata(&locked).unwrap().permissions();
    permissions.set_mode(0o555);
    fs::set_permissions(&locked, permissions).unwrap();

    let result = super::atomic_write(&locked.join("acceptances.toml"), b"x");

    let mut permissions = fs::metadata(&locked).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&locked, permissions).unwrap();
    assert!(result.unwrap_err().contains("cannot write"));
    fs::remove_dir_all(root).unwrap();
}


#[test]
fn record_propagates_inventory_and_existing_acceptance_errors() {
    let profile = ProfileContext::resolve(None, &[]).unwrap();
    let missing = std::env::temp_dir().join(format!(
        "ferric-lens-acceptance-missing-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&missing);
    assert!(record(&missing, &profile, "fingerprint", "reason", "digest")
        .unwrap_err()
        .contains("cannot resolve"));

    let root = temp_root();
    let digest = crate::input::inventory_with_profile(&root, &profile)
        .unwrap()
        .content_digest;
    fs::create_dir_all(root.join(".ferric-lens")).unwrap();
    fs::write(root.join(".ferric-lens/acceptances.toml"), "invalid = [").unwrap();

    assert!(record(&root, &profile, "fingerprint", "reason", &digest)
        .unwrap_err()
        .contains("invalid acceptance file"));
    fs::remove_dir_all(root).unwrap();
}
