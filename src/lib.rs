//! Ferric Lens analysis core.
//!
//! The implementation intentionally starts as one synchronous package. The
//! modules mirror the boundaries described in ARCHITECTURE.md without
//! introducing a framework or cross-module trait hierarchy prematurely.

pub mod acceptance;
pub mod architecture;
pub mod cache;
pub mod cfg;
pub mod compare;
pub mod correctness;
pub mod evidence;
pub mod extract;
pub mod git;
pub mod history;
pub mod input;
pub mod model;
pub mod profile;
pub mod report;
pub mod rules;

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use model::{
    AnalysisResult, BaselineContext, Capability, CapabilityStatus, GateVerdict, ModuleMetrics,
    Snapshot,
};

struct SnapshotAnalysis {
    content_digest: String,
    metadata_complete: bool,
    metadata_detail: Option<String>,
    cargo_input_digest: String,
    cargo_resolution_digest: Option<String>,
    auxiliary_targets: input::AuxiliaryTargetSummary,
    workspace_aliases: input::WorkspaceAliases,
    resolved_features_by_crate: BTreeMap<String, Vec<String>>,
    source_digests: BTreeMap<String, String>,
    modules: Vec<ModuleMetrics>,
    correctness_findings: Vec<model::Finding>,
    correctness_contexts: Vec<model::SourceContext>,
    parse_failures: usize,
}

type SourceContextOp = fn(
    &Path,
    &SnapshotAnalysis,
    &[model::Finding],
    &profile::ProfileContext,
) -> Result<Vec<model::SourceContext>, String>;

struct AnalysisOps {
    materialize_baseline: fn(&Path, &str) -> Result<git::TemporaryWorktree, String>,
    analyze_baseline:
        fn(&Path, &Path, &profile::ProfileContext) -> Result<SnapshotAnalysis, String>,
    sample_history: fn(&Path) -> Result<git::HistorySample, String>,
    source_contexts: SourceContextOp,
}

const REAL_OPS: AnalysisOps = AnalysisOps {
    materialize_baseline: git::materialize_worktree,
    analyze_baseline: analyze_snapshot,
    sample_history: git::sample_history,
    source_contexts: finding_source_contexts,
};

pub fn analyze(root: &Path) -> Result<AnalysisResult, String> {
    analyze_internal(root, None, true, None, None, &[])
}

pub fn analyze_with_base(root: &Path, base: Option<&str>) -> Result<AnalysisResult, String> {
    analyze_internal(root, base, true, None, None, &[])
}

pub fn analyze_with_base_and_evidence(
    root: &Path,
    base: Option<&str>,
    evidence_path: Option<&Path>,
) -> Result<AnalysisResult, String> {
    analyze_internal(root, base, true, evidence_path, None, &[])
}

pub fn analyze_with_profile(
    root: &Path,
    base: Option<&str>,
    target: Option<&str>,
    features: &[String],
    evidence_path: Option<&Path>,
) -> Result<AnalysisResult, String> {
    analyze_internal(root, base, true, evidence_path, target, features)
}

pub fn check_with_base(root: &Path, base: Option<&str>) -> Result<AnalysisResult, String> {
    analyze_internal(root, base, false, None, None, &[])
}

pub fn check_with_profile(
    root: &Path,
    base: Option<&str>,
    target: Option<&str>,
    features: &[String],
) -> Result<AnalysisResult, String> {
    analyze_internal(root, base, false, None, target, features)
}

fn analyze_internal(
    root: &Path,
    base: Option<&str>,
    include_history: bool,
    evidence_path: Option<&Path>,
    target: Option<&str>,
    features: &[String],
) -> Result<AnalysisResult, String> {
    analyze_internal_with_ops(
        root,
        base,
        include_history,
        evidence_path,
        target,
        features,
        &REAL_OPS,
    )
}

fn analyze_internal_with_ops(
    root: &Path,
    base: Option<&str>,
    include_history: bool,
    evidence_path: Option<&Path>,
    target: Option<&str>,
    features: &[String],
    ops: &AnalysisOps,
) -> Result<AnalysisResult, String> {
    let profile = profile::ProfileContext::resolve(target, features)?;
    analyze_internal_with_profile_context(root, base, include_history, evidence_path, &profile, ops)
}

fn analyze_internal_with_profile_context(
    root: &Path,
    base: Option<&str>,
    include_history: bool,
    evidence_path: Option<&Path>,
    profile: &profile::ProfileContext,
    ops: &AnalysisOps,
) -> Result<AnalysisResult, String> {
    let mut current = analyze_snapshot(root, root, profile)?;
    let architecture = architecture::summarize(&current.modules);
    let git_state = git::inspect(root);
    let snapshot = Snapshot {
        content_digest: current.content_digest.clone(),
        git_head: git_state.head,
        dirty: git_state.dirty,
        source_files: current.modules.len(),
    };

    let mut capabilities = snapshot_capabilities("head", &current);
    capabilities.push(Capability {
        name: "analysis_profile".into(),
        status: CapabilityStatus::Complete,
        detail: Some(format!(
            "{} resolved as {}; explicit features: {}",
            profile.public.target,
            profile.public.resolved_target,
            if profile.public.features.is_empty() {
                "none".to_owned()
            } else {
                profile.public.features.join(",")
            }
        )),
    });
    let imported_evidence = if let Some(path) = evidence_path {
        let imported = evidence::load(path, &snapshot, &profile.public)?;
        capabilities.push(Capability {
            name: "external_evidence".into(),
            status: if imported.attached {
                CapabilityStatus::Complete
            } else {
                CapabilityStatus::Partial
            },
            detail: Some(imported.attachment_reason.clone()),
        });
        Some(imported)
    } else {
        None
    };
    let mut findings = current.correctness_findings.clone();
    findings.extend(rules::current_snapshot_findings(&current.modules));
    refresh_refactor_candidates(&mut findings);
    finalize_findings(root, &profile.public.id, &mut findings)?;

    let baseline_selection = match git::resolve_baseline(root, base) {
        Ok(selection) => selection,
        Err(error) => {
            capabilities.push(Capability {
                name: "baseline_comparison".into(),
                status: CapabilityStatus::Unavailable,
                detail: Some(error.clone()),
            });
            capabilities.sort_by(|a, b| a.name.cmp(&b.name));
            let source_contexts = (ops.source_contexts)(root, &current, &findings, profile)?;
            return Ok(AnalysisResult {
                schema_version: 1,
                tool_version: env!("CARGO_PKG_VERSION").into(),
                snapshot,
                profile: profile.public.clone(),
                baseline: None,
                verdict: GateVerdict::Inconclusive,
                verdict_reason: format!("baseline comparison unavailable: {error}"),
                applicable_gate_subjects: 0,
                architecture: architecture.clone(),
                history: None,
                imported_evidence: imported_evidence.clone(),
                capabilities,
                modules: current.modules,
                source_contexts,
                findings,
            });
        }
    };

    let baseline_worktree = match (ops.materialize_baseline)(root, &baseline_selection.merge_base) {
        Ok(worktree) => worktree,
        Err(error) => {
            capabilities.push(Capability {
                name: "baseline_comparison".into(),
                status: CapabilityStatus::Unavailable,
                detail: Some(error.clone()),
            });
            capabilities.sort_by(|a, b| a.name.cmp(&b.name));
            let source_contexts = (ops.source_contexts)(root, &current, &findings, profile)?;
            return Ok(AnalysisResult {
                schema_version: 1,
                tool_version: env!("CARGO_PKG_VERSION").into(),
                snapshot,
                profile: profile.public.clone(),
                baseline: None,
                verdict: GateVerdict::Inconclusive,
                verdict_reason: format!("baseline materialization unavailable: {error}"),
                applicable_gate_subjects: 0,
                architecture: architecture.clone(),
                history: None,
                imported_evidence: imported_evidence.clone(),
                capabilities,
                modules: current.modules,
                source_contexts,
                findings,
            });
        }
    };

    let baseline_snapshot = match (ops.analyze_baseline)(baseline_worktree.path(), root, profile) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            capabilities.push(Capability {
                name: "baseline_comparison".into(),
                status: CapabilityStatus::Unavailable,
                detail: Some(error.clone()),
            });
            capabilities.sort_by(|a, b| a.name.cmp(&b.name));
            let source_contexts = (ops.source_contexts)(root, &current, &findings, profile)?;
            return Ok(AnalysisResult {
                schema_version: 1,
                tool_version: env!("CARGO_PKG_VERSION").into(),
                snapshot,
                profile: profile.public.clone(),
                baseline: None,
                verdict: GateVerdict::Inconclusive,
                verdict_reason: format!("baseline analysis failed: {error}"),
                applicable_gate_subjects: 0,
                architecture: architecture.clone(),
                history: None,
                imported_evidence: imported_evidence.clone(),
                capabilities,
                modules: current.modules,
                source_contexts,
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
    let baseline_advisories = rules::current_snapshot_findings(&baseline_snapshot.modules);
    rebind_advisory_baseline(
        &mut findings,
        &current.modules,
        &baseline_snapshot.modules,
        &correspondence,
        &baseline_advisories,
        &changes,
    );

    if !current.metadata_complete {
        gate.incomplete_reasons
            .push("head Cargo/source inventory is incomplete".into());
    }
    if !baseline_snapshot.metadata_complete {
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
    refresh_refactor_candidates(&mut findings);
    finalize_findings(root, &profile.public.id, &mut findings)?;
    sort_findings(&mut findings);

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
            match (ops.sample_history)(root) {
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
    let source_contexts = (ops.source_contexts)(root, &current, &findings, profile)?;

    Ok(AnalysisResult {
        schema_version: 1,
        tool_version: env!("CARGO_PKG_VERSION").into(),
        snapshot,
        profile: profile.public.clone(),
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
        architecture,
        history: history_summary,
        imported_evidence,
        capabilities,
        modules: current.modules,
        source_contexts,
        findings,
    })
}

pub fn accept_finding_with_base(
    root: &Path,
    base: Option<&str>,
    fingerprint: &str,
    reason: &str,
) -> Result<(), String> {
    accept_finding_with_profile(root, base, None, &[], fingerprint, reason)
}

pub fn accept_finding_with_profile(
    root: &Path,
    base: Option<&str>,
    target: Option<&str>,
    features: &[String],
    fingerprint: &str,
    reason: &str,
) -> Result<(), String> {
    let profile = profile::ProfileContext::resolve(target, features)?;
    let result =
        analyze_internal_with_profile_context(root, base, false, None, &profile, &REAL_OPS)?;
    if !result
        .findings
        .iter()
        .any(|finding| finding.fingerprint == fingerprint)
    {
        return Err(format!(
            "finding fingerprint {fingerprint} is not present in the current analysis"
        ));
    }

    acceptance::record(
        root,
        &profile,
        fingerprint,
        reason,
        &result.snapshot.content_digest,
    )
}

fn finding_source_contexts(
    root: &Path,
    snapshot: &SnapshotAnalysis,
    findings: &[model::Finding],
    profile: &profile::ProfileContext,
) -> Result<Vec<model::SourceContext>, String> {
    let mut contexts = extract::source_contexts_for_findings(
        root,
        &snapshot.modules,
        &snapshot.workspace_aliases,
        &snapshot.source_digests,
        findings,
        &profile.cfg,
        &snapshot.resolved_features_by_crate,
    )?;
    let requested = findings
        .iter()
        .flat_map(|finding| {
            finding
                .evidence
                .iter()
                .map(move |evidence| (finding.subject.as_str(), evidence.metric.as_str()))
        })
        .collect::<BTreeSet<_>>();
    contexts.extend(
        snapshot
            .correctness_contexts
            .iter()
            .filter(|context| {
                requested.contains(&(context.subject.as_str(), context.metric.as_str()))
            })
            .cloned(),
    );
    contexts.sort();
    contexts.dedup();
    Ok(contexts)
}

fn refresh_refactor_candidates(findings: &mut Vec<model::Finding>) {
    findings.retain(|finding| !finding.rule.starts_with("refactor."));
    let candidates = rules::refactor_candidates(findings);
    findings.extend(candidates);
}

fn sort_findings(findings: &mut [model::Finding]) {
    findings.sort_by(|a, b| {
        let gate_order = b.gate.cmp(&a.gate);
        if gate_order == std::cmp::Ordering::Equal {
            (&a.rule, &a.subject).cmp(&(&b.rule, &b.subject))
        } else {
            gate_order
        }
    });
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

fn rebind_advisory_baseline(
    findings: &mut [model::Finding],
    head: &[ModuleMetrics],
    baseline: &[ModuleMetrics],
    correspondence: &compare::Correspondence,
    baseline_findings: &[model::Finding],
    changes: &git::ChangeSet,
) {
    for finding in findings.iter_mut().filter(|finding| {
        finding.delta == model::DeltaStatus::Current
            && !finding.rule.starts_with("correctness.")
            && !finding.rule.starts_with("refactor.")
    }) {
        let Some((head_index, head_module)) = head.iter().enumerate().find(|(_, module)| {
            display_subject(&module.crate_name, &module.module_path) == finding.subject
        }) else {
            continue;
        };

        if correspondence.ambiguous_head.contains(&head_index) {
            finding.delta = model::DeltaStatus::Unknown;
            continue;
        }

        if let Some(&baseline_index) = correspondence.head_to_baseline.get(&head_index) {
            let previous = &baseline[baseline_index];
            let previous_subject = display_subject(&previous.crate_name, &previous.module_path);
            finding.identity = previous_subject.clone();
            let prior_finding = baseline_findings.iter().find(|baseline_finding| {
                baseline_finding.rule == finding.rule
                    && baseline_finding.subject == previous_subject
            });
            let worsened = advisory_evidence_materially_worsened(
                finding,
                prior_finding,
                previous,
                &previous_subject,
                baseline,
            );
            finding.delta = if worsened {
                model::DeltaStatus::Worsened
            } else if prior_finding.is_some() {
                model::DeltaStatus::Unchanged
            } else {
                model::DeltaStatus::Unknown
            };
            if matches!(
                finding.delta,
                model::DeltaStatus::Unchanged | model::DeltaStatus::Unknown
            ) && finding.priority == model::Priority::Investigate
            {
                finding.priority = model::Priority::Observe;
            }
            continue;
        }

        finding.delta = if changes.added.contains(&head_module.path) {
            model::DeltaStatus::New
        } else {
            model::DeltaStatus::Unknown
        };
    }
}

fn advisory_evidence_materially_worsened(
    current: &model::Finding,
    prior: Option<&model::Finding>,
    previous_module: &ModuleMetrics,
    previous_subject: &str,
    baseline: &[ModuleMetrics],
) -> bool {
    current.evidence.iter().any(|evidence| {
        let baseline_value = prior
            .and_then(|finding| {
                finding
                    .evidence
                    .iter()
                    .find(|item| item.metric == evidence.metric)
                    .map(|item| item.value)
            })
            .or_else(|| {
                advisory_baseline_metric_value(
                    &evidence.metric,
                    previous_module,
                    previous_subject,
                    baseline,
                )
            });

        baseline_value.is_some_and(|baseline_value| {
            advisory_growth_is_material(&evidence.metric, baseline_value, evidence.value)
        })
    })
}

fn advisory_baseline_metric_value(
    metric: &str,
    module: &ModuleMetrics,
    subject: &str,
    baseline: &[ModuleMetrics],
) -> Option<usize> {
    match metric {
        "decision_sites" => Some(module.decision_sites),
        "max_function_decision_sites" => Some(
            module
                .functions
                .iter()
                .map(|function| function.decision_sites)
                .max()
                .filter(|value| *value > 0)
                .unwrap_or(module.decision_sites),
        ),
        "local_dependency_modules" => Some(module.local_dependency_modules.len()),
        "clone_call_syntax_sites" => Some(module.clone_calls),
        "reverse_repository_dependents" => Some(
            baseline
                .iter()
                .filter(|candidate| {
                    candidate
                        .local_dependency_modules
                        .iter()
                        .any(|dependency| dependency == subject)
                })
                .count(),
        ),
        _ => None,
    }
}

fn advisory_growth_is_material(metric: &str, baseline: usize, current: usize) -> bool {
    if current <= baseline {
        return false;
    }

    let minimum = match metric {
        "decision_sites" | "max_function_decision_sites" => 3,
        "local_dependency_modules"
        | "clone_call_syntax_sites"
        | "reverse_repository_dependents" => 2,
        _ => 1,
    };
    current.saturating_sub(baseline) >= baseline.div_ceil(4).max(minimum)
}

fn display_subject(crate_name: &str, module_path: &str) -> String {
    if module_path.is_empty() {
        crate_name.to_owned()
    } else {
        format!("{crate_name}::{module_path}")
    }
}

fn finalize_findings(
    root: &Path,
    profile_id: &str,
    findings: &mut [model::Finding],
) -> Result<(), String> {
    for finding in findings.iter_mut() {
        finding.configuration = profile_id.to_owned();
    }
    acceptance::fingerprint_findings(findings);
    let acceptances = acceptance::load(root)?;
    acceptance::apply(findings, &acceptances);
    Ok(())
}

fn analyze_snapshot(
    root: &Path,
    cache_root: &Path,
    profile: &profile::ProfileContext,
) -> Result<SnapshotAnalysis, String> {
    let inventory = input::inventory_with_profile(root, profile)?;
    let correctness = correctness::scan(&inventory.sources);
    let source_digests = inventory
        .sources
        .iter()
        .map(|source| {
            (
                source.relative_path.clone(),
                blake3::hash(&source.bytes).to_hex().to_string(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let workspace_aliases = inventory.workspace_aliases.clone();
    let resolved_features_by_crate = inventory.resolved_features_by_crate.clone();
    let fact_cache = cache::RawFactCache::new(cache_root);
    let mut modules = Vec::with_capacity(inventory.sources.len());
    let mut parse_failures = 0usize;

    for source in &inventory.sources {
        let cfg = resolved_features_by_crate
            .get(&source.crate_name)
            .map(|features| profile.cfg.with_resolved_features(features))
            .unwrap_or_else(|| profile.cfg.clone());
        if let Some(module) = fact_cache.load(source, cfg.digest()) {
            modules.push(module);
            continue;
        }

        match extract::extract(source, &cfg) {
            Ok(module) => {
                fact_cache.store(source, cfg.digest(), &module);
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
    extract::resolve_workspace_dependencies(&mut modules, &workspace_aliases);

    Ok(SnapshotAnalysis {
        content_digest: inventory.content_digest,
        metadata_complete: inventory.metadata_complete,
        metadata_detail: inventory.metadata_detail,
        cargo_input_digest: inventory.cargo_input_digest,
        cargo_resolution_digest: inventory.cargo_resolution_digest,
        auxiliary_targets: inventory.auxiliary_targets,
        workspace_aliases,
        resolved_features_by_crate,
        source_digests,
        modules,
        correctness_findings: correctness.findings,
        correctness_contexts: correctness.source_contexts,
        parse_failures,
    })
}

fn snapshot_capabilities(side: &str, snapshot: &SnapshotAnalysis) -> Vec<Capability> {
    let cargo_metadata_available = snapshot.cargo_resolution_digest.is_some();
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
            name: format!("{side}.cargo_inputs"),
            status: CapabilityStatus::Complete,
            detail: Some(format!("digest {}", snapshot.cargo_input_digest)),
        },
        Capability {
            name: format!("{side}.cargo_resolution"),
            status: if cargo_metadata_available {
                CapabilityStatus::Complete
            } else {
                CapabilityStatus::Unavailable
            },
            detail: snapshot
                .cargo_resolution_digest
                .as_ref()
                .map(|digest| format!("digest {digest}"))
                .or_else(|| Some("Cargo metadata resolution unavailable".into())),
        },
        Capability {
            name: format!("{side}.source_scopes"),
            status: if cargo_metadata_available {
                CapabilityStatus::Complete
            } else {
                CapabilityStatus::Unavailable
            },
            detail: if cargo_metadata_available {
                Some(format!(
                    "tests={}, benches={}, examples={}, build_scripts={}, proc_macros={}",
                    snapshot.auxiliary_targets.tests,
                    snapshot.auxiliary_targets.benches,
                    snapshot.auxiliary_targets.examples,
                    snapshot.auxiliary_targets.build_scripts,
                    snapshot.auxiliary_targets.proc_macros
                ))
            } else {
                Some("auxiliary source classes unavailable without Cargo metadata".into())
            },
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

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
