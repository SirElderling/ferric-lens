use std::{
    fs,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

use crate::model::{AnalysisResult, Finding, GateVerdict};

static OUTPUT_COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn result_digest(result: &AnalysisResult) -> String {
    let bytes = serde_json::to_vec(result)
        .expect("analysis result schema contains only JSON-serializable values");
    blake3::hash(&bytes).to_hex().to_string()
}

pub fn json(result: &AnalysisResult) -> String {
    let digest = result_digest(result);
    let mut value =
        serde_json::to_value(result).expect("analysis result schema is JSON-serializable");
    let object = value
        .as_object_mut()
        .expect("analysis result serializes as a JSON object");
    object.insert("result_digest".into(), serde_json::Value::String(digest));
    serde_json::to_string_pretty(&value).expect("analysis result JSON value is serializable")
}

pub fn html(result: &AnalysisResult) -> String {
    let result_digest = result_digest(result);
    let verdict = match result.verdict {
        GateVerdict::Pass => "PASS",
        GateVerdict::Regression => "REGRESSION",
        GateVerdict::Inconclusive => "INCONCLUSIVE",
    };

    let mut capabilities = String::new();
    for capability in &result.capabilities {
        capabilities.push_str("<li><strong>");
        capabilities.push_str(&escape(&capability.name));
        capabilities.push_str("</strong>: ");
        capabilities.push_str(&escape(&format!("{:?}", capability.status).to_lowercase()));
        if let Some(detail) = &capability.detail {
            capabilities.push_str(" — ");
            capabilities.push_str(&escape(detail));
        }
        capabilities.push_str("</li>");
    }

    let gate_findings = result
        .findings
        .iter()
        .filter(|finding| finding.gate)
        .collect::<Vec<_>>();
    let advisory_findings = result
        .findings
        .iter()
        .filter(|finding| !finding.gate)
        .collect::<Vec<_>>();

    let gate_html = render_findings(&gate_findings, "No gate regression was established.");
    let advisory_html = render_findings(
        &advisory_findings,
        "No advisory coupled outliers were found in eligible crate populations.",
    );

    let mut modules = String::new();
    let mut current_crate = String::new();
    for module in &result.modules {
        if module.crate_name != current_crate {
            if !current_crate.is_empty() {
                modules.push_str("</div></details>");
            }
            current_crate = module.crate_name.clone();
            modules.push_str("<details><summary><strong>");
            modules.push_str(&escape(&current_crate));
            modules.push_str(r#"</strong></summary><div class="crate">"#);
        }
        modules.push_str(r#"<div class="module"><code>"#);
        modules.push_str(&escape(if module.module_path.is_empty() {
            "crate root"
        } else {
            &module.module_path
        }));
        modules.push_str("</code><span>");
        modules.push_str(&escape(&format!(
            "{} decisions · {} repo deps · {} public items · {} lines{}",
            module.decision_sites,
            module.local_dependency_modules.len(),
            module.public_items,
            module.lines,
            if module.gate_complete {
                ""
            } else {
                " · gate evidence incomplete"
            }
        )));
        modules.push_str("</span></div>");
        if let Some(history) = &module.history {
            modules.push_str(r#"<div class="history"><small>"#);
            modules.push_str(&escape(&format!(
                "history: changed in {} of {} sampled non-merge commits",
                history.change_commits, history.sampled_commits
            )));
            if !history.cochange.is_empty() {
                modules.push_str(" · co-change: ");
                for (index, related) in history.cochange.iter().enumerate() {
                    if index > 0 {
                        modules.push_str(", ");
                    }
                    modules.push_str("<code>");
                    modules.push_str(&escape(&related.path));
                    modules.push_str("</code> ");
                    modules.push_str(&escape(&format!("({})", related.shared_commits)));
                }
            }
            modules.push_str("</small></div>");
        }
    }
    if !current_crate.is_empty() {
        modules.push_str("</div></details>");
    }

    let profile_summary = format!(
        "<p><strong>{}</strong><br>resolved target <code>{}</code><br>{}<br>{} rustc cfg fact(s)</p>",
        escape(&result.profile.id),
        escape(&result.profile.resolved_target),
        if result.profile.features.is_empty() {
            "default features only".to_owned()
        } else {
            format!(
                "default + explicit features <code>{}</code>",
                escape(&result.profile.features.join(","))
            )
        },
        result.profile.target_cfg.len()
    );

    let history_summary = result.history.as_ref().map_or_else(
        || "<p>Not collected for this analysis mode.</p>".to_owned(),
        |history| {
            format!(
                "<p>{} sampled non-merge commits<br>{} changed-path records<br>{} broad commits excluded from co-change{}</p>",
                history.sampled_commits,
                history.changed_path_records,
                history.broad_commits_excluded_from_cochange,
                if history.truncated {
                    "<br><strong>Sample truncated at deterministic work limit.</strong>"
                } else {
                    ""
                }
            )
        },
    );

    let architecture_summary = if result.architecture.cycles.is_empty() {
        format!(
            "<p>{} modules<br>{} resolved explicit dependency edges<br>{} modules with incomplete graph evidence<br>No observed explicit-import cycles.</p>",
            result.architecture.modules,
            result.architecture.explicit_dependency_edges,
            result.architecture.incomplete_modules
        )
    } else {
        let mut html = format!(
            "<p>{} modules<br>{} resolved explicit dependency edges<br>{} modules with incomplete graph evidence<br><strong>{} observed explicit-import cycle(s)</strong></p><ul>",
            result.architecture.modules,
            result.architecture.explicit_dependency_edges,
            result.architecture.incomplete_modules,
            result.architecture.cycles.len()
        );
        for cycle in &result.architecture.cycles {
            html.push_str("<li>");
            html.push_str(&escape(&cycle.modules.join(" → ")));
            html.push_str("</li>");
        }
        html.push_str("</ul>");
        html
    };

    let imported_evidence = result.imported_evidence.as_ref().map_or_else(
        || "<p>No external evidence imported.</p>".to_owned(),
        |evidence| {
            let mut html = format!(
                "<p><strong>{}</strong> {}<br>{}<br><strong>{}</strong><br>target <code>{}</code>{}</p>",
                escape(&evidence.producer),
                escape(&evidence.producer_version),
                if evidence.attached {
                    "attached to current analysis"
                } else {
                    "unattached context only"
                },
                escape(&evidence.attachment_reason),
                escape(&evidence.target),
                if evidence.features.is_empty() {
                    String::new()
                } else {
                    format!("<br>features: <code>{}</code>", escape(&evidence.features.join(",")))
                }
            );
            if !evidence.observations.is_empty() {
                html.push_str("<details><summary>");
                html.push_str(&format!(
                    "{} imported observation(s)</summary><ul>",
                    evidence.observations.len()
                ));
                for observation in &evidence.observations {
                    html.push_str("<li><code>");
                    html.push_str(&escape(&observation.subject));
                    html.push_str("</code> · ");
                    html.push_str(&escape(&observation.metric));
                    html.push_str(" = ");
                    html.push_str(&escape(&format!(
                        "{} {}",
                        observation.value, observation.unit
                    )));
                    if let Some(note) = &observation.note {
                        html.push_str(" — ");
                        html.push_str(&escape(note));
                    }
                    html.push_str("</li>");
                }
                html.push_str("</ul></details>");
            }
            html
        },
    );

    let baseline = result.baseline.as_ref().map_or_else(
        || "<p>No comparable baseline was available.</p>".to_owned(),
        |baseline| {
            format!(
                "<p><strong>Target:</strong> <code>{}</code><br><strong>Merge base:</strong> <code>{}</code><br>{} baseline source files</p>",
                escape(&baseline.target_ref),
                escape(&baseline.merge_base),
                baseline.source_files
            )
        },
    );

    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Ferric Lens report</title>
<style>
:root {{ color-scheme: light dark; font-family: ui-sans-serif, system-ui, sans-serif; }}
body {{ max-width: 1100px; margin: 0 auto; padding: 2rem; line-height: 1.5; }}
header {{ border-bottom: 1px solid color-mix(in srgb, currentColor 20%, transparent); margin-bottom: 2rem; }}
.verdict {{ font-size: 1.5rem; font-weight: 700; }}
.grid {{ display: grid; grid-template-columns: repeat(auto-fit,minmax(260px,1fr)); gap: 1rem; }}
.card, article, details {{ border: 1px solid color-mix(in srgb, currentColor 20%, transparent); border-radius: .6rem; padding: 1rem; margin: .8rem 0; }}
article.gate {{ border-width: 2px; }}
summary {{ cursor: pointer; }}
.module {{ display: flex; justify-content: space-between; gap: 1rem; padding: .35rem 0; border-bottom: 1px solid color-mix(in srgb, currentColor 10%, transparent); }}
.module span {{ text-align: right; opacity: .8; }}
.history {{ margin: -.15rem 0 .5rem; padding-left: .5rem; opacity: .8; }}
code {{ overflow-wrap: anywhere; }}
</style>
</head>
<body>
<header><h1>Ferric Lens</h1><p class="verdict">{verdict}</p><p>{reason}</p></header>
<section class="grid">
<div class="card"><h2>Snapshot</h2><p><strong>Result digest</strong><br><code>{result_digest}</code></p><p><strong>Source digest</strong><br><code>{digest}</code></p><p>{source_files} source files<br>{applicable} applicable gate subjects</p></div>
<div class="card"><h2>Baseline</h2>{baseline}</div>
<div class="card"><h2>Profile</h2>{profile_summary}</div>
<div class="card"><h2>Architecture</h2>{architecture_summary}</div>
<div class="card"><h2>History</h2>{history_summary}</div>
<div class="card"><h2>External evidence</h2>{imported_evidence}</div>
<div class="card"><h2>Coverage</h2><ul>{capabilities}</ul></div>
</section>
<section><h2>Gate findings</h2>{gate_html}</section>
<section><h2>Advisory findings</h2>{advisory_html}</section>
<section><h2>Codebase map</h2>{modules}</section>
</body></html>"#,
        reason = escape(&result.verdict_reason),
        result_digest = escape(&result_digest),
        digest = escape(&result.snapshot.content_digest),
        source_files = result.snapshot.source_files,
        applicable = result.applicable_gate_subjects,
    )
}

fn render_findings(findings: &[&Finding], empty: &str) -> String {
    if findings.is_empty() {
        return format!("<p>{}</p>", escape(empty));
    }

    let mut html = String::new();
    for finding in findings {
        html.push_str(if finding.gate {
            r#"<article class="gate">"#
        } else {
            "<article>"
        });
        html.push_str("<h3>");
        html.push_str(&escape(&finding.subject));
        html.push_str("</h3><p><strong>");
        html.push_str(&escape(&finding.rule));
        html.push_str("</strong> · ");
        html.push_str(&escape(&format!("{:?}", finding.delta).to_lowercase()));
        if finding.accepted {
            html.push_str(" · accepted");
        }
        html.push_str("</p><p><strong>Fingerprint:</strong> <code>");
        html.push_str(&escape(&finding.fingerprint));
        html.push_str("</code></p>");
        if let Some(reason) = &finding.acceptance_reason {
            html.push_str("<p><strong>Acceptance:</strong> ");
            html.push_str(&escape(reason));
            html.push_str("</p>");
        }
        html.push_str("<p>");
        html.push_str(&escape(&finding.summary));
        html.push_str("</p><ul>");
        for evidence in &finding.evidence {
            html.push_str("<li>");
            let baseline = evidence
                .baseline
                .map(|value| format!(", baseline {value}"))
                .unwrap_or_default();
            let material = evidence
                .material_delta
                .map(|value| format!(", required growth {value}"))
                .unwrap_or_default();
            html.push_str(&escape(&format!(
                "{} = {} (p90 {}, population {}{}{})",
                evidence.metric,
                evidence.value,
                evidence.reference,
                evidence.population,
                baseline,
                material
            )));
            html.push_str("</li>");
        }
        html.push_str("</ul><p><strong>Direction:</strong> ");
        html.push_str(&escape(&finding.direction));
        html.push_str("</p></article>");
    }
    html
}

pub fn write(path: &Path, contents: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;

    let file_name = path
        .file_name()
        .ok_or_else(|| format!("output path has no file name: {}", path.display()))?
        .to_string_lossy();
    let counter = OUTPUT_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(".{file_name}.tmp-{}-{counter}", std::process::id()));

    fs::write(&temporary, contents)
        .map_err(|error| format!("cannot write {}: {error}", temporary.display()))?;
    match fs::rename(&temporary, path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            Err(format!("cannot replace {}: {error}", path.display()))
        }
    }
}

fn escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use crate::model::{
        AnalysisProfile, AnalysisResult, ArchitectureSummary, GateVerdict, Snapshot,
    };

    use super::{escape, html, json, result_digest};

    #[test]
    fn escapes_html_metacharacters() {
        assert_eq!(escape("<a x='&'>\""), "&lt;a x=&#39;&amp;&#39;&gt;&quot;");
    }

    fn minimal_result() -> AnalysisResult {
        AnalysisResult {
            schema_version: 1,
            tool_version: "test".into(),
            snapshot: Snapshot {
                content_digest: "source".into(),
                git_head: None,
                dirty: Some(false),
                source_files: 0,
            },
            profile: AnalysisProfile {
                id: "host".into(),
                target: "host".into(),
                resolved_target: "x86_64-unknown-linux-gnu".into(),
                features: Vec::new(),
                target_cfg: Vec::new(),
            },
            baseline: None,
            verdict: GateVerdict::Pass,
            verdict_reason: "complete".into(),
            applicable_gate_subjects: 0,
            architecture: ArchitectureSummary {
                modules: 0,
                explicit_dependency_edges: 0,
                incomplete_modules: 0,
                cycles: Vec::new(),
            },
            history: None,
            imported_evidence: None,
            capabilities: Vec::new(),
            modules: Vec::new(),
            findings: Vec::new(),
        }
    }

    #[test]
    fn renders_non_empty_report_sections_and_writes_atomically() {
        use std::fs;

        use crate::model::{
            AnalysisProfile, ArchitectureSummary, BaselineContext, Capability, CapabilityStatus,
            CoChangeEvidence, DependencyCycle, Evidence, EvidenceClass, Finding, HistoryEvidence,
            HistorySummary, ImportedEvidence, ImportedObservation, ModuleMetrics, Priority,
        };

        let mut result = minimal_result();
        result.verdict = GateVerdict::Regression;
        result.verdict_reason = "<regression>".into();
        result.profile = AnalysisProfile {
            id: "target=fixture;features=default+alpha".into(),
            target: "fixture".into(),
            resolved_target: "fixture-target".into(),
            features: vec!["alpha".into()],
            target_cfg: vec!["target_os=fixture".into()],
        };
        result.baseline = Some(BaselineContext {
            target_ref: "main".into(),
            target_oid: "a".repeat(40),
            merge_base: "b".repeat(40),
            source_files: 2,
            content_digest: "baseline".into(),
        });
        result.capabilities = vec![Capability {
            name: "syntax".into(),
            status: CapabilityStatus::Partial,
            detail: Some("<limited>".into()),
        }];
        result.architecture = ArchitectureSummary {
            modules: 2,
            explicit_dependency_edges: 2,
            incomplete_modules: 1,
            cycles: vec![DependencyCycle {
                modules: vec!["demo::a".into(), "demo::b".into()],
            }],
        };
        result.history = Some(HistorySummary {
            sampled_commits: 3,
            changed_path_records: 4,
            broad_commits_excluded_from_cochange: 1,
            truncated: true,
        });
        result.imported_evidence = Some(ImportedEvidence {
            producer: "bench".into(),
            producer_version: "1".into(),
            source_content_digest: Some("other".into()),
            source_git_commit: None,
            target: "fixture".into(),
            features: vec!["alpha".into()],
            attached: false,
            attachment_reason: "different source".into(),
            observations: vec![
                ImportedObservation {
                    subject: "src/a.rs".into(),
                    metric: "instructions".into(),
                    value: 7,
                    unit: "count".into(),
                    note: Some("<note>".into()),
                },
                ImportedObservation {
                    subject: "src/b.rs".into(),
                    metric: "allocations".into(),
                    value: 0,
                    unit: "count".into(),
                    note: None,
                },
            ],
        });
        result.modules = vec![
            ModuleMetrics {
                crate_name: "demo".into(),
                module_path: String::new(),
                path: "src/lib.rs".into(),
                lines: 10,
                decision_sites: 2,
                public_items: 1,
                explicit_imports: Vec::new(),
                local_dependency_modules: vec!["demo::a".into()],
                structure_digest: "root".into(),
                parse_complete: true,
                gate_complete: false,
                limitation: Some("macro".into()),
                history: None,
            },
            ModuleMetrics {
                crate_name: "demo".into(),
                module_path: "a".into(),
                path: "src/a.rs".into(),
                lines: 20,
                decision_sites: 4,
                public_items: 2,
                explicit_imports: Vec::new(),
                local_dependency_modules: Vec::new(),
                structure_digest: "a".into(),
                parse_complete: true,
                gate_complete: true,
                limitation: None,
                history: Some(HistoryEvidence {
                    change_commits: 2,
                    sampled_commits: 3,
                    cochange: vec![
                        CoChangeEvidence {
                            path: "src/b.rs".into(),
                            shared_commits: 2,
                        },
                        CoChangeEvidence {
                            path: "src/c.rs".into(),
                            shared_commits: 1,
                        },
                    ],
                }),
            },
        ];
        result.findings = vec![
            Finding {
                fingerprint: "gate".into(),
                configuration: "fixture".into(),
                rule: "rule.gate".into(),
                subject: "demo::a".into(),
                identity: "demo::a".into(),
                evidence_class: EvidenceClass::Strong,
                priority: Priority::ActFirst,
                delta: crate::model::DeltaStatus::Worsened,
                gate: true,
                accepted: true,
                acceptance_reason: Some("<accepted>".into()),
                summary: "<summary>".into(),
                direction: "<direction>".into(),
                evidence: vec![Evidence {
                    metric: "decisions".into(),
                    value: 10,
                    reference: 5,
                    population: 20,
                    baseline: Some(6),
                    material_delta: Some(3),
                }],
            },
            Finding {
                fingerprint: "advisory".into(),
                configuration: "fixture".into(),
                rule: "rule.advisory".into(),
                subject: "demo".into(),
                identity: "demo".into(),
                evidence_class: EvidenceClass::Candidate,
                priority: Priority::Observe,
                delta: crate::model::DeltaStatus::Current,
                gate: false,
                accepted: false,
                acceptance_reason: None,
                summary: "observe".into(),
                direction: "inspect".into(),
                evidence: Vec::new(),
            },
        ];

        let rendered = html(&result);
        assert!(rendered.contains("REGRESSION"));
        assert!(rendered.contains("observed explicit-import cycle"));
        assert!(rendered.contains("Sample truncated"));
        assert!(rendered.contains("unattached context only"));
        assert!(rendered.contains("&lt;accepted&gt;"));
        assert!(rendered.contains("co-change"));

        let path = std::env::temp_dir().join(format!(
            "ferric-lens-report-test-{}-nested/report.txt",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(path.parent().unwrap());
        super::write(&path, "first").unwrap();
        super::write(&path, "second").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "second");
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn write_rejects_paths_without_file_names() {
        let error = super::write(std::path::Path::new("/"), "content").unwrap_err();
        assert!(error.contains("output path has no file name"));
    }

    #[test]
    fn json_and_html_embed_the_same_semantic_result_digest() {
        let result = minimal_result();
        let digest = result_digest(&result);
        let json = json(&result);
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(value["result_digest"], digest);
        assert!(html(&result).contains(&digest));
    }

    #[test]
    fn renders_inconclusive_attached_evidence_multiple_crates_and_complete_history() {
        use crate::model::{HistoryEvidence, HistorySummary, ImportedEvidence, ModuleMetrics};

        let mut result = minimal_result();
        result.verdict = GateVerdict::Inconclusive;
        result.verdict_reason = "incomplete".into();
        result.history = Some(HistorySummary {
            sampled_commits: 1,
            changed_path_records: 1,
            broad_commits_excluded_from_cochange: 0,
            truncated: false,
        });
        result.imported_evidence = Some(ImportedEvidence {
            producer: "fixture".into(),
            producer_version: "1".into(),
            source_content_digest: Some("source".into()),
            source_git_commit: None,
            target: "host".into(),
            features: Vec::new(),
            attached: true,
            attachment_reason: "match".into(),
            observations: Vec::new(),
        });
        result.modules = vec![
            ModuleMetrics {
                crate_name: "a".into(),
                module_path: String::new(),
                path: "a/src/lib.rs".into(),
                lines: 1,
                decision_sites: 0,
                public_items: 0,
                explicit_imports: Vec::new(),
                local_dependency_modules: Vec::new(),
                structure_digest: "a".into(),
                parse_complete: true,
                gate_complete: true,
                limitation: None,
                history: Some(HistoryEvidence {
                    change_commits: 1,
                    sampled_commits: 1,
                    cochange: Vec::new(),
                }),
            },
            ModuleMetrics {
                crate_name: "b".into(),
                module_path: String::new(),
                path: "b/src/lib.rs".into(),
                lines: 1,
                decision_sites: 0,
                public_items: 0,
                explicit_imports: Vec::new(),
                local_dependency_modules: Vec::new(),
                structure_digest: "b".into(),
                parse_complete: true,
                gate_complete: true,
                limitation: None,
                history: None,
            },
        ];

        let rendered = html(&result);

        assert!(rendered.contains("INCONCLUSIVE"));
        assert!(rendered.contains("attached to current analysis"));
        assert!(!rendered.contains("Sample truncated"));
        assert!(rendered.matches("<details><summary><strong>").count() >= 2);
    }

    #[test]
    fn imported_observation_without_a_note_renders_without_note_separator() {
        use crate::model::{ImportedEvidence, ImportedObservation};

        let mut result = minimal_result();
        result.imported_evidence = Some(ImportedEvidence {
            producer: "fixture".into(),
            producer_version: "1".into(),
            source_content_digest: Some("source".into()),
            source_git_commit: None,
            target: "host".into(),
            features: Vec::new(),
            attached: true,
            attachment_reason: "match".into(),
            observations: vec![ImportedObservation {
                subject: "src/lib.rs".into(),
                metric: "instructions".into(),
                value: 1,
                unit: "count".into(),
                note: None,
            }],
        });

        let rendered = html(&result);

        assert!(rendered.contains("instructions = 1 count"));
        assert!(!rendered.contains("instructions = 1 count —"));
    }

    #[test]
    fn write_reports_parent_creation_failure() {
        use std::fs;

        let root = std::env::temp_dir().join(format!(
            "ferric-lens-report-parent-error-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let blocker = root.join("blocker");
        fs::write(&blocker, "file").unwrap();

        let error = super::write(&blocker.join("report.json"), "x").unwrap_err();

        assert!(error.contains("cannot create"));
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn write_reports_temporary_write_failure() {
        use std::{fs, os::unix::fs::PermissionsExt};

        let root = std::env::temp_dir().join(format!(
            "ferric-lens-report-write-error-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let mut permissions = fs::metadata(&root).unwrap().permissions();
        permissions.set_mode(0o555);
        fs::set_permissions(&root, permissions).unwrap();

        let result = super::write(&root.join("report.json"), "x");

        let mut permissions = fs::metadata(&root).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&root, permissions).unwrap();
        assert!(result.unwrap_err().contains("cannot write"));
        fs::remove_dir_all(root).unwrap();
    }
}
