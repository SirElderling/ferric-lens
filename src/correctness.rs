// ferric-lens: ignore-correctness-risks
use crate::{
    input::SourceFile,
    model::{DeltaStatus, Evidence, EvidenceClass, Finding, Priority, SourceContext},
};

#[derive(Debug, Default)]
pub struct CorrectnessScan {
    pub findings: Vec<Finding>,
    pub source_contexts: Vec<SourceContext>,
}

#[derive(Debug, Clone)]
struct Match {
    path: String,
    line: usize,
    excerpt: String,
}

const CARGO_NO_DEPS: &str = concat!("--no", "-deps");
const FEATURE_KEY_BRANCH: &str = concat!("key == ", "\"feature\"");
const SYMBOLIC_TARGET_IDENTITY: &str = concat!("target={target_", "label};features=");
const SYMBOLIC_PROFILE_TARGET: &str = concat!("profile.", "target");
const ROOT_ONLY_CARGO_LOOP: &str = concat!("for name in [\"Cargo.", "toml\", \"Cargo.lock\"]");
const WORKSPACE_MANIFEST_PATH: &str = concat!("package.", "manifest_path");
const GIT_COMMAND: &str = concat!("Command::new(", "\"git\")");
const LOSSY_RECORD: &str = concat!("String::from_utf8_lossy(", "record)");
const LOSSY_PATH: &str = concat!("String::from_utf8_lossy(", "path)");

const SUPPRESSION_DIRECTIVE: &str = "ferric-lens: ignore-correctness-risks";

pub fn scan(sources: &[SourceFile]) -> CorrectnessScan {
    let filtered = sources
        .iter()
        .filter(|source| {
            std::str::from_utf8(&source.bytes)
                .map(|text| !text.contains(SUPPRESSION_DIRECTIVE))
                .unwrap_or(true)
        })
        .cloned()
        .collect::<Vec<_>>();
    let sources = filtered.as_slice();

    let mut scan = CorrectnessScan::default();
    detect_cargo_feature_resolution(sources, &mut scan);
    detect_symbolic_target_identity(sources, &mut scan);
    detect_stdout_mode_artifacts(sources, &mut scan);
    detect_workspace_manifest_snapshot_gap(sources, &mut scan);
    detect_lossy_git_paths(sources, &mut scan);
    scan.findings
        .sort_by(|a, b| (&a.rule, &a.subject).cmp(&(&b.rule, &b.subject)));
    scan.source_contexts.sort();
    scan.source_contexts.dedup();
    scan
}

fn detect_cargo_feature_resolution(sources: &[SourceFile], scan: &mut CorrectnessScan) {
    let no_deps = matches(sources, CARGO_NO_DEPS);
    let feature_branch = matches(sources, FEATURE_KEY_BRANCH);
    let explicit_features = matches(sources, "explicit_features");
    if no_deps.is_empty() || feature_branch.is_empty() || explicit_features.is_empty() {
        return;
    }

    push_finding(
        scan,
        "correctness.cargo_feature_resolution_without_resolve",
        "repository",
        EvidenceClass::Strong,
        Priority::ActFirst,
        "Cargo feature cfg is inferred without Cargo's resolved per-package feature graph",
        "collect Cargo metadata with the resolve graph available, map enabled features per workspace package, and evaluate cfg(feature) against that package-specific resolved set; keep feature reachability unknown when resolution is unavailable",
        vec![
            evidence("cargo_metadata_no_deps_sites", no_deps.len()),
            evidence("cfg_feature_resolution_sites", feature_branch.len()),
        ],
        vec![
            ("cargo_metadata_no_deps_sites", no_deps),
            ("cfg_feature_resolution_sites", feature_branch),
        ],
    );
}

fn detect_symbolic_target_identity(sources: &[SourceFile], scan: &mut CorrectnessScan) {
    let symbolic = matches(sources, SYMBOLIC_TARGET_IDENTITY);
    let symbolic_match = matches(sources, SYMBOLIC_PROFILE_TARGET);
    let resolved = matches(sources, "resolved_target");
    if symbolic.is_empty() || symbolic_match.is_empty() || resolved.is_empty() {
        return;
    }

    push_finding(
        scan,
        "correctness.symbolic_target_identity",
        "repository",
        EvidenceClass::Strong,
        Priority::ActFirst,
        "Machine configuration identity uses the symbolic host label instead of the resolved target",
        "use the resolved target triple in persistent finding/acceptance identity and in imported-evidence configuration matching; keep the symbolic host label only for display",
        vec![
            evidence("symbolic_target_identity_sites", symbolic.len()),
            evidence("symbolic_target_match_sites", symbolic_match.len()),
        ],
        vec![
            ("symbolic_target_identity_sites", symbolic),
            ("symbolic_target_match_sites", symbolic_match),
        ],
    );
}

fn detect_stdout_mode_artifacts(sources: &[SourceFile], scan: &mut CorrectnessScan) {
    for source in sources {
        let Ok(text) = std::str::from_utf8(&source.bytes) else {
            continue;
        };
        let lines = text.lines().collect::<Vec<_>>();
        for (index, line) in lines.iter().enumerate() {
            if !line.contains("output_text(&result, ai)") {
                continue;
            }
            let start = index.saturating_sub(12);
            let window = &lines[start..=index];
            let guarded = window.iter().any(|line| {
                line.contains("if ai") || line.contains("if !ai") || line.contains("if ! ai")
            });
            if guarded {
                continue;
            }
            let writes = window
                .iter()
                .enumerate()
                .filter(|(_, line)| {
                    line.contains("report::write(&json") || line.contains("report::write(&html")
                })
                .map(|(offset, line)| Match {
                    path: source.relative_path.clone(),
                    line: start + offset + 1,
                    excerpt: line.trim().to_owned(),
                })
                .collect::<Vec<_>>();
            if writes.len() < 2 {
                continue;
            }

            push_finding(
                scan,
                "correctness.stdout_mode_unconditional_artifacts",
                "repository",
                EvidenceClass::Strong,
                Priority::ActFirst,
                "A stdout-oriented output mode still performs unconditional default artifact writes",
                "make default artifact paths conditional on human/file output mode; in compact stdout/AI mode, write files only when the caller explicitly requested those output paths",
                vec![evidence("unconditional_output_write_sites", writes.len())],
                vec![("unconditional_output_write_sites", writes)],
            );
            return;
        }
    }
}

fn detect_workspace_manifest_snapshot_gap(sources: &[SourceFile], scan: &mut CorrectnessScan) {
    let root_only = matches(sources, ROOT_ONLY_CARGO_LOOP);
    let member_manifest = matches(sources, WORKSPACE_MANIFEST_PATH);
    let verifier = matches(sources, "verify_stable_inputs");
    if root_only.is_empty() || member_manifest.is_empty() || verifier.is_empty() {
        return;
    }

    push_finding(
        scan,
        "correctness.workspace_manifest_snapshot_gap",
        "repository",
        EvidenceClass::Strong,
        Priority::ActFirst,
        "Snapshot stability verification does not cover all workspace manifests used during analysis",
        "capture the complete set of workspace-member manifests used by Cargo metadata, include them in the initial Cargo-input digest, and verify the same files again before publishing the snapshot",
        vec![
            evidence("root_only_cargo_input_sites", root_only.len()),
            evidence("workspace_manifest_read_sites", member_manifest.len()),
        ],
        vec![
            ("root_only_cargo_input_sites", root_only),
            ("workspace_manifest_read_sites", member_manifest),
        ],
    );
}

fn detect_lossy_git_paths(sources: &[SourceFile], scan: &mut CorrectnessScan) {
    let mut lossy = Vec::new();
    for source in sources {
        let Ok(text) = std::str::from_utf8(&source.bytes) else {
            continue;
        };
        if !text.contains(GIT_COMMAND) {
            continue;
        }
        for (index, line) in text.lines().enumerate() {
            if line.contains(LOSSY_RECORD) || line.contains(LOSSY_PATH) {
                lossy.push(Match {
                    path: source.relative_path.clone(),
                    line: index + 1,
                    excerpt: line.trim().to_owned(),
                });
            }
        }
    }
    if lossy.is_empty() {
        return;
    }

    push_finding(
        scan,
        "correctness.lossy_git_path_decoding",
        "repository",
        EvidenceClass::Strong,
        Priority::ActFirst,
        "Git repository paths are decoded with a lossy UTF-8 conversion",
        "preserve Git path bytes/OS strings where possible; otherwise reject non-UTF-8 repository paths explicitly and mark the affected analysis capability incomplete instead of silently replacing bytes",
        vec![evidence("lossy_git_path_decode_sites", lossy.len())],
        vec![("lossy_git_path_decode_sites", lossy)],
    );
}

fn matches(sources: &[SourceFile], needle: &str) -> Vec<Match> {
    let mut out = Vec::new();
    for source in sources {
        let Ok(text) = std::str::from_utf8(&source.bytes) else {
            continue;
        };
        for (index, line) in text.lines().enumerate() {
            if line.contains(needle) {
                out.push(Match {
                    path: source.relative_path.clone(),
                    line: index + 1,
                    excerpt: line.trim().to_owned(),
                });
            }
        }
    }
    out
}

fn evidence(metric: &str, value: usize) -> Evidence {
    Evidence {
        metric: metric.to_owned(),
        value,
        reference: 0,
        population: 1,
        baseline: None,
        material_delta: None,
    }
}

#[allow(clippy::too_many_arguments)]
fn push_finding(
    scan: &mut CorrectnessScan,
    rule: &str,
    subject: &str,
    evidence_class: EvidenceClass,
    priority: Priority,
    summary: &str,
    direction: &str,
    evidence: Vec<Evidence>,
    contexts: Vec<(&str, Vec<Match>)>,
) {
    scan.findings.push(Finding {
        fingerprint: String::new(),
        rule: rule.to_owned(),
        subject: subject.to_owned(),
        identity: subject.to_owned(),
        configuration: String::new(),
        evidence_class,
        priority,
        delta: DeltaStatus::Current,
        gate: false,
        accepted: false,
        acceptance_reason: None,
        summary: summary.to_owned(),
        direction: direction.to_owned(),
        evidence,
    });

    for (metric, matches) in contexts {
        for matched in matches.into_iter().take(3) {
            scan.source_contexts.push(SourceContext {
                subject: subject.to_owned(),
                metric: metric.to_owned(),
                path: matched.path,
                start_line: matched.line,
                end_line: matched.line,
                excerpt: matched.excerpt,
                excerpt_truncated: false,
            });
        }
    }
}

#[cfg(test)]
#[path = "correctness_tests.rs"]
mod tests;
