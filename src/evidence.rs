use std::{
    fs,
    path::{Component, Path},
};

use serde::Deserialize;

use crate::model::{AnalysisProfile, ImportedEvidence, ImportedObservation, Snapshot};

const MAX_IMPORT_BYTES: u64 = 16 * 1024 * 1024;
const IMPORT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Deserialize)]
struct Envelope {
    schema_version: u32,
    producer: Producer,
    source: SourceIdentity,
    configuration: Configuration,
    observations: Vec<Observation>,
}

#[derive(Debug, Deserialize)]
struct Producer {
    name: String,
    version: String,
}

#[derive(Debug, Deserialize)]
struct SourceIdentity {
    content_digest: Option<String>,
    git_commit: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Configuration {
    target: String,
    #[serde(default)]
    features: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Observation {
    subject: String,
    metric: String,
    value: i64,
    unit: String,
    note: Option<String>,
}

pub fn load(
    path: &Path,
    snapshot: &Snapshot,
    profile: &AnalysisProfile,
) -> Result<ImportedEvidence, String> {
    let metadata = fs::metadata(path)
        .map_err(|error| format!("cannot inspect evidence import {}: {error}", path.display()))?;
    if metadata.len() > MAX_IMPORT_BYTES {
        return Err(format!(
            "evidence import {} exceeds the 16 MiB limit",
            path.display()
        ));
    }

    let bytes = fs::read(path)
        .map_err(|error| format!("cannot read evidence import {}: {error}", path.display()))?;
    let mut envelope: Envelope = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid evidence import JSON: {error}"))?;

    validate_envelope(&envelope)?;

    envelope.configuration.features.sort();
    envelope.configuration.features.dedup();
    envelope.observations.sort_by(|left, right| {
        (
            &left.subject,
            &left.metric,
            &left.unit,
            left.value,
            &left.note,
        )
            .cmp(&(
                &right.subject,
                &right.metric,
                &right.unit,
                right.value,
                &right.note,
            ))
    });

    let source_matches = envelope
        .source
        .content_digest
        .as_deref()
        .is_some_and(|digest| digest == snapshot.content_digest)
        || (snapshot.dirty == Some(false)
            && envelope
                .source
                .git_commit
                .as_deref()
                .zip(snapshot.git_head.as_deref())
                .is_some_and(|(expected, actual)| expected == actual));

    let configuration_matches = envelope.configuration.target == profile.target
        && envelope.configuration.features == profile.features;

    let (attached, attachment_reason) = match (source_matches, configuration_matches) {
        (true, true) => (
            true,
            "source identity and analysis profile match the current snapshot".to_owned(),
        ),
        (false, true) => (
            false,
            "source identity does not match the current snapshot".to_owned(),
        ),
        (true, false) => (
            false,
            "configuration does not match the current analysis profile".to_owned(),
        ),
        (false, false) => (
            false,
            "source identity and configuration do not match the current analysis".to_owned(),
        ),
    };

    Ok(ImportedEvidence {
        producer: envelope.producer.name,
        producer_version: envelope.producer.version,
        source_content_digest: envelope.source.content_digest,
        source_git_commit: envelope.source.git_commit,
        target: envelope.configuration.target,
        features: envelope.configuration.features,
        attached,
        attachment_reason,
        observations: envelope
            .observations
            .into_iter()
            .map(|observation| ImportedObservation {
                subject: observation.subject,
                metric: observation.metric,
                value: observation.value,
                unit: observation.unit,
                note: observation.note,
            })
            .collect(),
    })
}

fn validate_envelope(envelope: &Envelope) -> Result<(), String> {
    if envelope.schema_version != IMPORT_SCHEMA_VERSION {
        return Err(format!(
            "unsupported evidence schema version {}; expected {}",
            envelope.schema_version, IMPORT_SCHEMA_VERSION
        ));
    }

    if envelope.producer.name.trim().is_empty() || envelope.producer.version.trim().is_empty() {
        return Err("evidence producer name and version must be non-empty".into());
    }

    if envelope.source.content_digest.is_none() && envelope.source.git_commit.is_none() {
        return Err("evidence source must include content_digest or git_commit".into());
    }

    if envelope.configuration.target.trim().is_empty() {
        return Err("evidence configuration target must be non-empty".into());
    }

    for feature in &envelope.configuration.features {
        if feature.trim().is_empty() {
            return Err("evidence feature names must be non-empty".into());
        }
    }

    for observation in &envelope.observations {
        if observation.subject.trim().is_empty()
            || observation.metric.trim().is_empty()
            || observation.unit.trim().is_empty()
        {
            return Err("evidence observation subject, metric, and unit must be non-empty".into());
        }
        validate_subject(&observation.subject)?;
    }

    Ok(())
}

fn validate_subject(subject: &str) -> Result<(), String> {
    let path = Path::new(subject);
    if path.is_absolute() {
        return Err(format!(
            "evidence subject must be repository-relative: {subject}"
        ));
    }

    for component in path.components() {
        if matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        ) {
            return Err(format!(
                "evidence subject escapes repository-relative scope: {subject}"
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
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
}
