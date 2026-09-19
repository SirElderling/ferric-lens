use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use crate::model::{AnalysisProfile, Snapshot};

use super::load;

static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_file(contents: &str) -> PathBuf {
    let counter = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "ferric-lens-evidence-test-{}-{counter}.json",
        std::process::id()
    ));
    fs::write(&path, contents).unwrap();
    path
}

fn profile() -> AnalysisProfile {
    AnalysisProfile {
        id: "target=host;features=default".into(),
        target: "host".into(),
        resolved_target: "x86_64-unknown-linux-gnu".into(),
        features: Vec::new(),
        target_cfg: Vec::new(),
    }
}

fn snapshot() -> Snapshot {
    Snapshot {
        content_digest: "digest".into(),
        git_head: Some("abc123".into()),
        dirty: Some(false),
        source_files: 1,
    }
}

#[test]
fn attaches_matching_host_default_evidence() {
    let path = temp_file(
        r#"{
          "schema_version": 1,
          "producer": {"name": "bench", "version": "1"},
          "source": {"content_digest": "digest"},
          "configuration": {"target": "host", "features": []},
          "observations": [
            {"subject": "src/lib.rs", "metric": "instructions", "value": 42, "unit": "count"}
          ]
        }"#,
    );

    let imported = load(&path, &snapshot(), &profile()).unwrap();
    assert!(imported.attached);
    assert_eq!(imported.observations.len(), 1);
    fs::remove_file(path).unwrap();
}

#[test]
fn retains_mismatched_source_as_unattached_context() {
    let path = temp_file(
        r#"{
          "schema_version": 1,
          "producer": {"name": "bench", "version": "1"},
          "source": {"content_digest": "other"},
          "configuration": {"target": "host", "features": []},
          "observations": []
        }"#,
    );

    let imported = load(&path, &snapshot(), &profile()).unwrap();
    assert!(!imported.attached);
    fs::remove_file(path).unwrap();
}

#[test]
fn rejects_subjects_that_escape_repository_scope() {
    let path = temp_file(
        r#"{
          "schema_version": 1,
          "producer": {"name": "bench", "version": "1"},
          "source": {"content_digest": "digest"},
          "configuration": {"target": "host", "features": []},
          "observations": [
            {"subject": "../outside", "metric": "x", "value": 1, "unit": "count"}
          ]
        }"#,
    );

    assert!(load(&path, &snapshot(), &profile()).is_err());
    fs::remove_file(path).unwrap();
}

#[test]
fn attaches_clean_git_commit_identity_and_normalizes_features_and_observations() {
    let path = temp_file(
        r#"{
          "schema_version": 1,
          "producer": {"name": "bench", "version": "1"},
          "source": {"git_commit": "abc123"},
          "configuration": {"target": "host", "features": ["z", "a", "a"]},
          "observations": [
            {"subject": "src/z.rs", "metric": "m", "value": 2, "unit": "count"},
            {"subject": "src/a.rs", "metric": "m", "value": 1, "unit": "count", "note": "n"}
          ]
        }"#,
    );
    let mut profile = profile();
    profile.features = vec!["a".into(), "z".into()];

    let imported = load(&path, &snapshot(), &profile).unwrap();

    assert!(imported.attached);
    assert_eq!(imported.features, ["a", "z"]);
    assert_eq!(imported.observations[0].subject, "src/a.rs");
    fs::remove_file(path).unwrap();
}

#[test]
fn distinguishes_configuration_and_combined_mismatches() {
    let configuration_only = temp_file(
        r#"{"schema_version":1,"producer":{"name":"p","version":"1"},"source":{"content_digest":"digest"},"configuration":{"target":"other","features":[]},"observations":[]}"#,
    );
    let both = temp_file(
        r#"{"schema_version":1,"producer":{"name":"p","version":"1"},"source":{"content_digest":"other"},"configuration":{"target":"other","features":[]},"observations":[]}"#,
    );

    let one = load(&configuration_only, &snapshot(), &profile()).unwrap();
    let two = load(&both, &snapshot(), &profile()).unwrap();

    assert_eq!(
        one.attachment_reason,
        "configuration does not match the current analysis profile"
    );
    assert_eq!(
        two.attachment_reason,
        "source identity and configuration do not match the current analysis"
    );
    fs::remove_file(configuration_only).unwrap();
    fs::remove_file(both).unwrap();
}

#[test]
fn rejects_invalid_envelope_contracts() {
    let cases = [
        (
            r#"{"schema_version":2,"producer":{"name":"p","version":"1"},"source":{"content_digest":"d"},"configuration":{"target":"host"},"observations":[]}"#,
            "unsupported evidence schema version",
        ),
        (
            r#"{"schema_version":1,"producer":{"name":"","version":"1"},"source":{"content_digest":"d"},"configuration":{"target":"host"},"observations":[]}"#,
            "producer name and version",
        ),
        (
            r#"{"schema_version":1,"producer":{"name":"p","version":"1"},"source":{},"configuration":{"target":"host"},"observations":[]}"#,
            "source must include",
        ),
        (
            r#"{"schema_version":1,"producer":{"name":"p","version":"1"},"source":{"content_digest":"d"},"configuration":{"target":""},"observations":[]}"#,
            "target must be non-empty",
        ),
        (
            r#"{"schema_version":1,"producer":{"name":"p","version":"1"},"source":{"content_digest":"d"},"configuration":{"target":"host","features":[" "]},"observations":[]}"#,
            "feature names must be non-empty",
        ),
        (
            r#"{"schema_version":1,"producer":{"name":"p","version":"1"},"source":{"content_digest":"d"},"configuration":{"target":"host"},"observations":[{"subject":"","metric":"m","value":1,"unit":"u"}]}"#,
            "observation subject, metric, and unit",
        ),
        (
            r#"{"schema_version":1,"producer":{"name":"p","version":"1"},"source":{"content_digest":"d"},"configuration":{"target":"host"},"observations":[{"subject":"/absolute","metric":"m","value":1,"unit":"u"}]}"#,
            "repository-relative",
        ),
    ];

    for (json, expected) in cases {
        let path = temp_file(json);
        let error = load(&path, &snapshot(), &profile()).unwrap_err();
        assert!(error.contains(expected), "{error}");
        fs::remove_file(path).unwrap();
    }
}

#[test]
fn reports_missing_and_malformed_imports() {
    let missing = std::env::temp_dir().join(format!(
        "ferric-lens-evidence-missing-{}.json",
        std::process::id()
    ));
    let _ = fs::remove_file(&missing);
    assert!(load(&missing, &snapshot(), &profile())
        .unwrap_err()
        .contains("cannot inspect evidence import"));

    let malformed = temp_file("{");
    assert!(load(&malformed, &snapshot(), &profile())
        .unwrap_err()
        .contains("invalid evidence import JSON"));
    fs::remove_file(malformed).unwrap();
}

#[test]
fn rejects_oversized_import_before_reading_contents() {
    let counter = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "ferric-lens-evidence-large-{}-{counter}.json",
        std::process::id()
    ));
    let file = fs::File::create(&path).unwrap();
    file.set_len(super::MAX_IMPORT_BYTES + 1).unwrap();

    let error = load(&path, &snapshot(), &profile()).unwrap_err();

    assert!(error.contains("exceeds the 16 MiB limit"));
    fs::remove_file(path).unwrap();
}
