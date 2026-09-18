//! Ferric Lens analysis core.
//!
//! The implementation intentionally starts as one synchronous package. The
//! modules mirror the boundaries described in ARCHITECTURE.md without
//! introducing a framework or cross-module trait hierarchy prematurely.

pub mod acceptance;
pub mod cache;
pub mod compare;
pub mod extract;
pub mod git;
pub mod history;
pub mod input;
pub mod model;
pub mod report;
pub mod rules;

use std::{collections::BTreeSet, path::Path};

use model::{
    AnalysisResult, BaselineContext, Capability, CapabilityStatus, GateVerdict, ModuleMetrics,
    Snapshot,
};

struct SnapshotAnalysis {
    content_digest: String,
    metadata_complete: bool,
    metadata_detail: Option<String>,
    modules: Vec<ModuleMetrics>,
    parse_failures: usize,
}

pub fn analyze(root: &Path) -> Result<AnalysisResult, String> {
    analyze_internal(root, None, true)
}

pub fn analyze_with_base(root: &Path, base: Option<&str>) -> Result<AnalysisResult, String> {
    analyze_internal(root, base, true)
}

pub fn check_with_base(root: &Path, base: Option<&str>) -> Result<AnalysisResult, String> {
    analyze_internal(root, base, false)
}

fn analyze_internal(
    root: &Path,
    base: Option<&str>,
    include_history: bool,
) -> Result<AnalysisResult, String> {
    let mut current = analyze_snapshot(root, root)?;
    let git_state = git::inspect(root);
    let snapshot = Snapshot {
        content_digest: current.content_digest.clone(),
        git_head: git_state.head,
        dirty: git_state.dirty,
        source_files: current.modules.len(),
    };

    let mut capabilities = snapshot_capabilities("head", &current);
    let mut findings = rules::current_snapshot_findings(&current.modules);
    finalize_findings(root, &mut findings)?;

    let baseline_selection = match git::resolve_baseline(root, base) {
        Ok(selection) => selection,
        Err(error) => {
            capabilities.push(Capability {
                name: "baseline_comparison".into(),
                status: CapabilityStatus::Unavailable,
                detail: Some(error.clone()),
            });
            capabilities.sort_by(|a, b| a.name.cmp(&b.name));
            return Ok(AnalysisResult {
                schema_version: 1,
                tool_version: env!("CARGO_PKG_VERSION").into(),
                snapshot,
                baseline: None,
                verdict: GateVerdict::Inconclusive,
                verdict_reason: format!("baseline comparison unavailable: {error}"),
                applicable_gate_subjects: 0,
                history: None,
                capabilities,
                modules: current.modules,
                findings,
            });
        }
    };

    let baseline_worktree = match git::materialize_worktree(root, &baseline_selection.merge_base) {
        Ok(worktree) => worktree,
        Err(error) => {
            capabilities.push(Capability {
                name: "baseline_comparison".into(),
                status: CapabilityStatus::Unavailable,
                detail: Some(error.clone()),
            });
            capabilities.sort_by(|a, b| a.name.cmp(&b.name));
            return Ok(AnalysisResult {
                schema_version: 1,
                tool_version: env!("CARGO_PKG_VERSION").into(),
                snapshot,
                baseline: None,
                verdict: GateVerdict::Inconclusive,
                verdict_reason: format!("baseline materialization unavailable: {error}"),
                applicable_gate_subjects: 0,
                history: None,
                capabilities,
                modules: current.modules,
                findings,
            });
        }
    };

    let baseline_snapshot = match analyze_snapshot(baseline_worktree.path(), root) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            capabilities.push(Capability {
                name: "baseline_comparison".into(),
                status: CapabilityStatus::Unavailable,
                detail: Some(error.clone()),
            });
            capabilities.sort_by(|a, b| a.name.cmp(&b.name));
            return Ok(AnalysisResult {
                schema_version: 1,
                tool_version: env!("CARGO_PKG_VERSION").into(),
                snapshot,
                baseline: None,
                verdict: GateVerdict::Inconclusive,
                verdict_reason: format!("baseline analysis failed: {error}"),
                applicable_gate_subjects: 0,
                history: None,
                capabilities,
                modules: current.modules,
                findings,
            });
        }
    };

    let changes = git::changes_since(root, &baseline_selection.merge_base)?;
    let correspondence =
        compare::match_modules(&baseline_snapshot.modules, &current.modules, &changes);
    let mut gate = rules::evaluate_regressions(
        &baseline_snapshot.modules,
        &current.modules,
        &changes,
        &correspondence,
    );
    rebind_advisory_identities(
        &mut findings,
        &current.modules,
        &baseline_snapshot.modules,
        &correspondence,
    );

    if !current.metadata_complete && !current.modules.is_empty() {
        gate.incomplete_reasons
            .push("head Cargo/source inventory is incomplete".into());
    }
    if !baseline_snapshot.metadata_complete && !baseline_snapshot.modules.is_empty() {
        gate.incomplete_reasons
            .push("baseline Cargo/source inventory is incomplete".into());
    }
    if current.parse_failures > 0 {
        gate.incomplete_reasons.push(format!(
            "head has {} unparsed Rust source file(s)",
            current.parse_failures
        ));
    }
    if baseline_snapshot.parse_failures > 0 {
        gate.incomplete_reasons.push(format!(
            "baseline has {} unparsed Rust source file(s)",
            baseline_snapshot.parse_failures
        ));
    }
    gate.incomplete_reasons.sort();
    gate.incomplete_reasons.dedup();

    findings.extend(gate.findings);
    finalize_findings(root, &mut findings)?;
    findings.sort_by(|a, b| {
        b.gate
            .cmp(&a.gate)
            .then_with(|| (&a.rule, &a.subject).cmp(&(&b.rule, &b.subject)))
    });

    let has_regression = findings
        .iter()
        .any(|finding| finding.gate && !finding.accepted);
    let (verdict, verdict_reason) = if has_regression {
        (
            GateVerdict::Regression,
            "at least one unaccepted gate regression was established".to_owned(),
        )
    } else if !gate.incomplete_reasons.is_empty() {
        (
            GateVerdict::Inconclusive,
            format!(
                "required gate evidence is incomplete: {}",
                gate.incomplete_reasons.join("; ")
            ),
        )
    } else if gate.applicable_subjects == 0 {
        (
            GateVerdict::Pass,
            "enabled gate checks completed; no gate subjects were applicable".to_owned(),
        )
    } else {
        (
            GateVerdict::Pass,
            format!(
                "enabled gate checks completed for {} applicable subject(s); no regression established",
                gate.applicable_subjects
            ),
        )
    };

    capabilities.extend(snapshot_capabilities("baseline", &baseline_snapshot));
    capabilities.push(Capability {
        name: "baseline_comparison".into(),
        status: if gate.incomplete_reasons.is_empty() {
            CapabilityStatus::Complete
        } else {
            CapabilityStatus::Partial
        },
        detail: if gate.incomplete_reasons.is_empty() {
            Some(format!(
                "target {} at {}, merge base {}",
                baseline_selection.target_ref,
                short_oid(&baseline_selection.target_oid),
                short_oid(&baseline_selection.merge_base)
            ))
        } else {
            Some(gate.incomplete_reasons.join("; "))
        },
    });
    let history_summary = if include_history {
        let candidates = history_candidates(&current.modules, &changes, &findings);
        if candidates.is_empty() {
            capabilities.push(Capability {
                name: "history_enrichment".into(),
                status: CapabilityStatus::Complete,
                detail: Some(
                    "no changed or finding-related production paths required enrichment".into(),
                ),
            });
            None
        } else {
            match git::sample_history(root) {
                Ok(sample) => {
                    let summary = history::enrich(&mut current.modules, &sample, &candidates);
                    capabilities.push(Capability {
                        name: "history_enrichment".into(),
                        status: if summary.truncated {
                            CapabilityStatus::Partial
                        } else {
                            CapabilityStatus::Complete
                        },
                        detail: Some(format!(
                            "{} non-merge commits sampled; {} changed-path records; {} broad commits excluded from co-change{}",
                            summary.sampled_commits,
                            summary.changed_path_records,
                            summary.broad_commits_excluded_from_cochange,
                            if summary.truncated { "; sample truncated at deterministic work limit" } else { "" }
                        )),
                    });
                    Some(summary)
                }
                Err(error) => {
                    capabilities.push(Capability {
                        name: "history_enrichment".into(),
                        status: CapabilityStatus::Unavailable,
                        detail: Some(error),
                    });
                    None
                }
            }
        }
    } else {
        None
    };

    capabilities.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(AnalysisResult {
        schema_version: 1,
        tool_version: env!("CARGO_PKG_VERSION").into(),
        snapshot,
        baseline: Some(BaselineContext {
            target_ref: baseline_selection.target_ref,
            target_oid: baseline_selection.target_oid,
            merge_base: baseline_selection.merge_base,
            source_files: baseline_snapshot.modules.len(),
            content_digest: baseline_snapshot.content_digest,
        }),
        verdict,
        verdict_reason,
        applicable_gate_subjects: gate.applicable_subjects,
        history: history_summary,
        capabilities,
        modules: current.modules,
        findings,
    })
}

pub fn accept_finding_with_base(
    root: &Path,
    base: Option<&str>,
    fingerprint: &str,
    reason: &str,
) -> Result<(), String> {
    let result = check_with_base(root, base)?;
    if !result
        .findings
        .iter()
        .any(|finding| finding.fingerprint == fingerprint)
    {
        return Err(format!(
            "finding fingerprint {fingerprint} is not present in the current analysis"
        ));
    }

    acceptance::record(root, fingerprint, reason, &result.snapshot.content_digest)
}

fn history_candidates(
    modules: &[ModuleMetrics],
    changes: &git::ChangeSet,
    findings: &[model::Finding],
) -> BTreeSet<String> {
    let module_paths = modules
        .iter()
        .map(|module| module.path.as_str())
        .collect::<BTreeSet<_>>();

    let mut candidates = BTreeSet::new();
    for path in changes
        .added
        .iter()
        .chain(changes.modified.iter())
        .chain(changes.renames.values())
    {
        if module_paths.contains(path.as_str()) {
            candidates.insert(path.clone());
        }
    }

    for finding in findings {
        if let Some(module) = modules.iter().find(|module| {
            display_subject(&module.crate_name, &module.module_path) == finding.subject
        }) {
            candidates.insert(module.path.clone());
        }
    }

    candidates
}

fn rebind_advisory_identities(
    findings: &mut [model::Finding],
    head: &[ModuleMetrics],
    baseline: &[ModuleMetrics],
    correspondence: &compare::Correspondence,
) {
    for finding in findings
        .iter_mut()
        .filter(|finding| finding.rule == "structure.current_coupled_outlier")
    {
        let Some((head_index, _)) = head.iter().enumerate().find(|(_, module)| {
            display_subject(&module.crate_name, &module.module_path) == finding.subject
        }) else {
            continue;
        };
        let Some(&baseline_index) = correspondence.head_to_baseline.get(&head_index) else {
            continue;
        };
        let previous = &baseline[baseline_index];
        finding.identity = display_subject(&previous.crate_name, &previous.module_path);
    }
}

fn display_subject(crate_name: &str, module_path: &str) -> String {
    if module_path.is_empty() {
        crate_name.to_owned()
    } else {
        format!("{crate_name}::{module_path}")
    }
}

fn finalize_findings(root: &Path, findings: &mut [model::Finding]) -> Result<(), String> {
    acceptance::fingerprint_findings(findings);
    let acceptances = acceptance::load(root)?;
    acceptance::apply(findings, &acceptances);
    Ok(())
}

fn analyze_snapshot(root: &Path, cache_root: &Path) -> Result<SnapshotAnalysis, String> {
    let inventory = input::inventory(root)?;
    let fact_cache = cache::RawFactCache::new(cache_root);
    let mut modules = Vec::with_capacity(inventory.sources.len());
    let mut parse_failures = 0usize;

    for source in &inventory.sources {
        if let Some(module) = fact_cache.load(source) {
            modules.push(module);
            continue;
        }

        match extract::extract(source) {
            Ok(module) => {
                fact_cache.store(source, &module);
                modules.push(module);
            }
            Err(error) => {
                parse_failures += 1;
                modules.push(ModuleMetrics::unsupported(
                    source.crate_name.clone(),
                    source.module_path.clone(),
                    source.relative_path.clone(),
                    error,
                ));
            }
        }
    }

    modules.sort_by(|a, b| {
        (&a.crate_name, &a.module_path, &a.path).cmp(&(&b.crate_name, &b.module_path, &b.path))
    });
    extract::resolve_workspace_dependencies(&mut modules, &inventory.workspace_aliases);

    Ok(SnapshotAnalysis {
        content_digest: inventory.content_digest,
        metadata_complete: inventory.metadata_complete,
        metadata_detail: inventory.metadata_detail,
        modules,
        parse_failures,
    })
}

fn snapshot_capabilities(side: &str, snapshot: &SnapshotAnalysis) -> Vec<Capability> {
    vec![
        Capability {
            name: format!("{side}.source_inventory"),
            status: if snapshot.metadata_complete {
                CapabilityStatus::Complete
            } else {
                CapabilityStatus::Partial
            },
            detail: snapshot.metadata_detail.clone(),
        },
        Capability {
            name: format!("{side}.syntax_extraction"),
            status: if snapshot.parse_failures == 0 {
                CapabilityStatus::Complete
            } else {
                CapabilityStatus::Partial
            },
            detail: if snapshot.parse_failures == 0 {
                None
            } else {
                Some(format!(
                    "{} Rust source file(s) could not be parsed",
                    snapshot.parse_failures
                ))
            },
        },
    ]
}

fn short_oid(oid: &str) -> &str {
    oid.get(..12).unwrap_or(oid)
}
