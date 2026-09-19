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
pub mod evidence;
pub mod extract;
pub mod git;
pub mod history;
pub mod input;
pub mod model;
pub mod profile;
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
    let profile = profile::ProfileContext::resolve(target, features)?;
    let mut current = analyze_snapshot(root, root, &profile)?;
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
    let mut findings = rules::current_snapshot_findings(&current.modules);
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
                findings,
            });
        }
    };

    let baseline_snapshot = match analyze_snapshot(baseline_worktree.path(), root, &profile) {
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
    finalize_findings(root, &profile.public.id, &mut findings)?;
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
    let result = check_with_profile(root, base, target, features)?;
    if !result
        .findings
        .iter()
        .any(|finding| finding.fingerprint == fingerprint)
    {
        return Err(format!(
            "finding fingerprint {fingerprint} is not present in the current analysis"
        ));
    }

    let profile = profile::ProfileContext::resolve(target, features)?;
    acceptance::record(
        root,
        &profile,
        fingerprint,
        reason,
        &result.snapshot.content_digest,
    )
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
    let fact_cache = cache::RawFactCache::new(cache_root);
    let mut modules = Vec::with_capacity(inventory.sources.len());
    let mut parse_failures = 0usize;

    for source in &inventory.sources {
        if let Some(module) = fact_cache.load(source, profile.cfg.digest()) {
            modules.push(module);
            continue;
        }

        match extract::extract(source, &profile.cfg) {
            Ok(module) => {
                fact_cache.store(source, profile.cfg.digest(), &module);
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


#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use crate::{
        compare::Correspondence,
        git::ChangeSet,
        model::{
            CapabilityStatus, DeltaStatus, EvidenceClass, Finding, ModuleMetrics, Priority,
        },
    };

    fn module(crate_name: &str, module_path: &str, path: &str) -> ModuleMetrics {
        ModuleMetrics {
            crate_name: crate_name.into(),
            module_path: module_path.into(),
            path: path.into(),
            lines: 1,
            decision_sites: 0,
            public_items: 0,
            explicit_imports: Vec::new(),
            local_dependency_modules: Vec::new(),
            structure_digest: path.into(),
            parse_complete: true,
            gate_complete: true,
            limitation: None,
            history: None,
        }
    }

    fn finding(rule: &str, subject: &str) -> Finding {
        Finding {
            fingerprint: String::new(),
            rule: rule.into(),
            subject: subject.into(),
            identity: subject.into(),
            configuration: String::new(),
            evidence_class: EvidenceClass::Candidate,
            priority: Priority::Observe,
            delta: DeltaStatus::Current,
            gate: false,
            accepted: false,
            acceptance_reason: None,
            summary: String::new(),
            direction: String::new(),
            evidence: Vec::new(),
        }
    }

    #[test]
    fn history_candidates_include_changed_renamed_and_finding_paths_only_when_owned() {
        let modules = vec![
            module("demo", "", "src/lib.rs"),
            module("demo", "a", "src/a.rs"),
            module("demo", "b", "src/b.rs"),
        ];
        let changes = ChangeSet {
            added: BTreeSet::from(["src/a.rs".into(), "README.md".into()]),
            modified: BTreeSet::from(["src/lib.rs".into()]),
            deleted: BTreeSet::new(),
            renames: BTreeMap::from([("src/old.rs".into(), "src/b.rs".into())]),
        };
        let findings = vec![finding("advisory", "demo::a"), finding("other", "missing")];

        let candidates = super::history_candidates(&modules, &changes, &findings);

        assert_eq!(
            candidates,
            BTreeSet::from([
                "src/a.rs".to_owned(),
                "src/b.rs".to_owned(),
                "src/lib.rs".to_owned(),
            ])
        );
    }

    #[test]
    fn advisory_identity_rebinds_only_with_matching_subject_and_correspondence() {
        let head = vec![module("demo", "moved", "src/moved.rs")];
        let baseline = vec![module("demo", "old", "src/old.rs")];
        let mut findings = vec![
            finding("structure.current_coupled_outlier", "demo::moved"),
            finding("structure.current_coupled_outlier", "missing"),
            finding("other", "demo::moved"),
        ];
        let correspondence = Correspondence {
            head_to_baseline: [(0, 0)].into_iter().collect(),
            ambiguous_head: Default::default(),
        };

        super::rebind_advisory_identities(
            &mut findings,
            &head,
            &baseline,
            &correspondence,
        );

        assert_eq!(findings[0].identity, "demo::old");
        assert_eq!(findings[1].identity, "missing");
        assert_eq!(findings[2].identity, "demo::moved");

        let mut no_match = vec![finding(
            "structure.current_coupled_outlier",
            "demo::moved",
        )];
        super::rebind_advisory_identities(
            &mut no_match,
            &head,
            &baseline,
            &Correspondence::default(),
        );
        assert_eq!(no_match[0].identity, "demo::moved");
    }

    #[test]
    fn display_subject_and_short_oid_cover_root_and_short_values() {
        assert_eq!(super::display_subject("demo", ""), "demo");
        assert_eq!(super::display_subject("demo", "a"), "demo::a");
        assert_eq!(super::short_oid("1234567890123456"), "123456789012");
        assert_eq!(super::short_oid("short"), "short");
    }

    #[test]
    fn snapshot_capabilities_reflect_complete_and_partial_syntax_and_inventory() {
        let complete = super::SnapshotAnalysis {
            content_digest: "digest".into(),
            metadata_complete: true,
            metadata_detail: None,
            modules: Vec::new(),
            parse_failures: 0,
        };
        let complete_caps = super::snapshot_capabilities("head", &complete);
        assert!(complete_caps
            .iter()
            .all(|capability| capability.status == CapabilityStatus::Complete));

        let partial = super::SnapshotAnalysis {
            content_digest: "digest".into(),
            metadata_complete: false,
            metadata_detail: Some("metadata unavailable".into()),
            modules: Vec::new(),
            parse_failures: 2,
        };
        let partial_caps = super::snapshot_capabilities("baseline", &partial);
        assert!(partial_caps
            .iter()
            .all(|capability| capability.status == CapabilityStatus::Partial));
        assert!(partial_caps
            .iter()
            .any(|capability| capability.detail.as_deref() == Some("2 Rust source file(s) could not be parsed")));
    }
