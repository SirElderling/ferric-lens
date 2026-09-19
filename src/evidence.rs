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

    let configuration_matches = envelope.configuration.target == profile.resolved_target
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
#[path = "evidence_tests.rs"]
mod tests;
