use std::{
    fs,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

use crate::model::{
    AnalysisResult, DeltaStatus, EvidenceClass, Finding, GateVerdict, ModuleMetrics, Priority,
    SourceContext,
};

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
    let refactor_advisories = result
        .findings
        .iter()
        .filter(|finding| !finding.gate && finding.rule.starts_with("refactor."))
        .collect::<Vec<_>>();
    let structural_advisories = result
        .findings
        .iter()
        .filter(|finding| !finding.gate && finding.rule.starts_with("structure."))
        .collect::<Vec<_>>();
    let runtime_advisories = result
        .findings
        .iter()
        .filter(|finding| !finding.gate && finding.rule.starts_with("runtime."))
        .collect::<Vec<_>>();
    let build_advisories = result
        .findings
        .iter()
        .filter(|finding| !finding.gate && finding.rule.starts_with("build."))
        .collect::<Vec<_>>();
    let other_advisories = result
        .findings
        .iter()
        .filter(|finding| {
            !finding.gate
                && !finding.rule.starts_with("refactor.")
                && !finding.rule.starts_with("structure.")
                && !finding.rule.starts_with("runtime.")
                && !finding.rule.starts_with("build.")
        })
        .collect::<Vec<_>>();

    let gate_html = render_findings(
        &gate_findings,
        "No gate regression was established.",
        &result.modules,
        &result.source_contexts,
    );
    let refactor_html = render_findings(
        &refactor_advisories,
        "No multi-signal refactoring candidates were established.",
        &result.modules,
        &result.source_contexts,
    );
    let structural_html = render_findings(
        &structural_advisories,
        "No structural advisory outliers were found in eligible populations.",
        &result.modules,
        &result.source_contexts,
    );
    let runtime_html = render_findings(
        &runtime_advisories,
        "No static runtime-risk candidates were found in eligible populations.",
        &result.modules,
        &result.source_contexts,
    );
    let build_html = render_findings(
        &build_advisories,
        "No build-efficiency candidates were found in eligible populations.",
        &result.modules,
        &result.source_contexts,
    );
    let other_html = render_findings(
        &other_advisories,
        "No other advisory findings.",
        &result.modules,
        &result.source_contexts,
    );
    let triage_summary = render_triage_summary(result);

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
        let subject = module_subject(module);
        let anchor = anchor_id("module", &subject);
        modules.push_str(r#"<div class="module-block" id=""#);
        modules.push_str(&anchor);
        modules.push_str(r#""><div class="module"><code>"#);
        modules.push_str(&escape(if module.module_path.is_empty() {
            "crate root"
        } else {
            &module.module_path
        }));
        modules.push_str("</code><span>");
        modules.push_str(&escape(&format!(
            "{} decisions · {} repo deps · {} public items · {} functions · {} types · {} clone syntax sites · {} lines{}",
            module.decision_sites,
            module.local_dependency_modules.len(),
            module.public_items,
            module.functions.len(),
            module.types.len(),
            module.clone_calls,
            module.lines,
            if module.gate_complete {
                ""
            } else {
                " · gate evidence incomplete"
            }
        )));
        modules.push_str("</span></div>");

        if !module.local_dependency_modules.is_empty() {
            modules.push_str("<details><summary>Dependencies (");
            modules.push_str(&module.local_dependency_modules.len().to_string());
            modules.push_str(")</summary><ul>");
            for dependency in &module.local_dependency_modules {
                modules.push_str("<li><code>");
                modules.push_str(&escape(dependency));
                modules.push_str("</code></li>");
            }
            modules.push_str("</ul></details>");
        }

        if !module.functions.is_empty() {
            modules.push_str("<details><summary>Functions (");
            modules.push_str(&module.functions.len().to_string());
            modules.push_str(")</summary><ul>");
            for function in &module.functions {
                modules.push_str("<li><code>");
                modules.push_str(&escape(&function.name));
                modules.push_str("</code> · ");
                modules.push_str(&escape(&format!("{:?}", function.kind).to_lowercase()));
                modules.push_str(" · ");
                modules.push_str(if function.public_declared {
                    "public"
                } else {
                    "private"
                });
                modules.push_str("</li>");
            }
            modules.push_str("</ul></details>");
        }

        if !module.types.is_empty() {
            modules.push_str("<details><summary>Types (");
            modules.push_str(&module.types.len().to_string());
            modules.push_str(")</summary><ul>");
            for item_type in &module.types {
                modules.push_str("<li><code>");
                modules.push_str(&escape(&item_type.name));
                modules.push_str("</code> · ");
                modules.push_str(&escape(&format!("{:?}", item_type.kind).to_lowercase()));
                modules.push_str(" · ");
                modules.push_str(if item_type.public_declared {
                    "public"
                } else {
                    "private"
                });
                modules.push_str("</li>");
            }
            modules.push_str("</ul></details>");
        }

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
        modules.push_str("</div>");
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
.module-block {{ scroll-margin-top: 1rem; }}
.module {{ display: flex; justify-content: space-between; gap: 1rem; padding: .35rem 0; border-bottom: 1px solid color-mix(in srgb, currentColor 10%, transparent); }}
.module span {{ text-align: right; opacity: .8; }}
.finding-meta, .location {{ opacity: .85; }}
.source-evidence {{ margin: .8rem 0; }}
.source-context {{ margin: .7rem 0; }}
.source-context pre {{ margin: .35rem 0; padding: .7rem; overflow-x: auto; white-space: pre-wrap; background: color-mix(in srgb, currentColor 6%, transparent); border-radius: .4rem; }}
.history {{ margin: -.15rem 0 .5rem; padding-left: .5rem; opacity: .8; }}
code {{ overflow-wrap: anywhere; }}
</style>
</head>
<body>
<header><h1>Ferric Lens</h1><p class="verdict">{verdict}</p><p>{reason}</p></header>
<section class="grid">
<div class="card"><h2>Triage</h2>{triage_summary}</div>
<div class="card"><h2>Snapshot</h2><p><strong>Result digest</strong><br><code>{result_digest}</code></p><p><strong>Source digest</strong><br><code>{digest}</code></p><p>{source_files} source files<br>{applicable} applicable gate subjects</p></div>
<div class="card"><h2>Baseline</h2>{baseline}</div>
<div class="card"><h2>Profile</h2>{profile_summary}</div>
<div class="card"><h2>Architecture</h2>{architecture_summary}</div>
<div class="card"><h2>History</h2>{history_summary}</div>
<div class="card"><h2>External evidence</h2>{imported_evidence}</div>
<div class="card"><h2>Coverage</h2><ul>{capabilities}</ul></div>
</section>
<section><h2>Gate findings</h2>{gate_html}</section>
<section><h2>Refactoring candidates</h2>{refactor_html}</section>
<section><h2>Structural advisories</h2>{structural_html}</section>
<section><h2>Runtime-risk candidates</h2>{runtime_html}</section>
<section><h2>Build-efficiency candidates</h2>{build_html}</section>
<section><h2>Other advisories</h2>{other_html}</section>
<section><h2>Codebase map</h2>{modules}</section>
</body></html>"#,
        reason = escape(&result.verdict_reason),
        result_digest = escape(&result_digest),
        digest = escape(&result.snapshot.content_digest),
        source_files = result.snapshot.source_files,
        applicable = result.applicable_gate_subjects,
    )
}

fn render_triage_summary(result: &AnalysisResult) -> String {
    let active = result
        .findings
        .iter()
        .filter(|finding| !finding.accepted)
        .collect::<Vec<_>>();
    let act_first = active
        .iter()
        .filter(|finding| finding.priority == Priority::ActFirst)
        .count();
    let investigate = active
        .iter()
        .filter(|finding| finding.priority == Priority::Investigate)
        .count();
    let observe = active
        .iter()
        .filter(|finding| finding.priority == Priority::Observe)
        .count();
    let refactors = active
        .iter()
        .filter(|finding| finding.rule.starts_with("refactor."))
        .count();
    let changed = active
        .iter()
        .filter(|finding| matches!(finding.delta, DeltaStatus::New | DeltaStatus::Worsened))
        .count();

    format!(
        "<p><strong>{}</strong><br>{}<br>{}<br>{}<br>{}</p>",
        count_phrase(act_first, "act first", "act first"),
        count_phrase(investigate, "investigate", "investigate"),
        count_phrase(observe, "observe", "observe"),
        count_phrase(refactors, "refactoring candidate", "refactoring candidates"),
        count_phrase(changed, "new/worsened finding", "new/worsened findings")
    )
}

fn count_phrase(count: usize, singular: &str, plural: &str) -> String {
    format!("{count} {}", if count == 1 { singular } else { plural })
}

fn render_findings(
    findings: &[&Finding],
    empty: &str,
    modules: &[ModuleMetrics],
    source_contexts: &[SourceContext],
) -> String {
    if findings.is_empty() {
        return format!("<p>{}</p>", escape(empty));
    }

    let mut ordered = findings.to_vec();
    ordered.sort_by(|a, b| {
        priority_rank(&a.priority)
            .cmp(&priority_rank(&b.priority))
            .then_with(|| delta_rank(&a.delta).cmp(&delta_rank(&b.delta)))
            .then_with(|| a.subject.cmp(&b.subject))
            .then_with(|| a.rule.cmp(&b.rule))
    });

    let mut html = String::new();
    for finding in ordered {
        html.push_str(if finding.gate {
            r#"<article class="gate">"#
        } else {
            "<article>"
        });
        html.push_str("<h3>");
        html.push_str(&escape(&finding.subject));
        html.push_str("</h3><p class=\"finding-meta\"><strong>");
        html.push_str(priority_label(&finding.priority));
        html.push_str("</strong> · ");
        html.push_str(evidence_label(&finding.evidence_class));
        html.push_str(" · ");
        html.push_str(delta_label(&finding.delta));
        if finding.accepted {
            html.push_str(" · accepted");
        }
        html.push_str("</p><p><strong>");
        html.push_str(&escape(&finding.rule));
        html.push_str("</strong></p>");

        if let Some(module) = modules
            .iter()
            .find(|module| module_subject(module) == finding.subject)
        {
            let anchor = anchor_id("module", &finding.subject);
            html.push_str(r#"<p class="location"><strong>Source:</strong> <code>"#);
            html.push_str(&escape(&module.path));
            html.push_str("</code> · <a href=\"#");
            html.push_str(&anchor);
            html.push_str("\">View module</a></p>");
        }

        let evidence_metrics = finding
            .evidence
            .iter()
            .map(|evidence| evidence.metric.as_str())
            .collect::<Vec<_>>();
        let contexts = source_contexts
            .iter()
            .filter(|context| {
                context.subject == finding.subject
                    && evidence_metrics.contains(&context.metric.as_str())
            })
            .collect::<Vec<_>>();
        if !contexts.is_empty() {
            html.push_str("<details class=\"source-evidence\"><summary>Source evidence (");
            html.push_str(&contexts.len().to_string());
            html.push_str(")</summary>");
            for context in contexts {
                html.push_str("<div class=\"source-context\"><p><strong>");
                html.push_str(&escape(&context.metric));
                html.push_str("</strong> · <code>");
                html.push_str(&escape(&source_location_label(context)));
                html.push_str("</code></p><pre><code>");
                html.push_str(&escape(&context.excerpt));
                html.push_str("</code></pre>");
                if context.excerpt_truncated {
                    html.push_str("<p><small>Excerpt bounded for report size; the exact source span is preserved above.</small></p>");
                }
                html.push_str("</div>");
            }
            html.push_str("</details>");
        }

        html.push_str("<p><strong>Fingerprint:</strong> <code>");
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

fn priority_rank(priority: &Priority) -> u8 {
    match priority {
        Priority::ActFirst => 0,
        Priority::Investigate => 1,
        Priority::Observe => 2,
    }
}

fn delta_rank(delta: &DeltaStatus) -> u8 {
    match delta {
        DeltaStatus::Worsened => 0,
        DeltaStatus::New => 1,
        DeltaStatus::Current => 2,
        DeltaStatus::Unchanged => 3,
        DeltaStatus::Unknown => 4,
    }
}

fn priority_label(priority: &Priority) -> &'static str {
    match priority {
        Priority::ActFirst => "Priority 1 — act first",
        Priority::Investigate => "Priority 2 — investigate",
        Priority::Observe => "Observe",
    }
}

fn evidence_label(evidence: &EvidenceClass) -> &'static str {
    match evidence {
        EvidenceClass::Proven => "proven evidence",
        EvidenceClass::Strong => "strong evidence",
        EvidenceClass::Candidate => "candidate evidence",
    }
}

fn delta_label(delta: &DeltaStatus) -> &'static str {
    match delta {
        DeltaStatus::Current => "current",
        DeltaStatus::New => "new",
        DeltaStatus::Worsened => "worsened",
        DeltaStatus::Unchanged => "unchanged",
        DeltaStatus::Unknown => "unknown",
    }
}

fn module_subject(module: &ModuleMetrics) -> String {
    if module.module_path.is_empty() {
        module.crate_name.clone()
    } else {
        format!("{}::{}", module.crate_name, module.module_path)
    }
}

fn anchor_id(prefix: &str, value: &str) -> String {
    let digest = blake3::hash(value.as_bytes()).to_hex().to_string();
    format!("{prefix}-{}", &digest[..16])
}

fn source_location_label(context: &SourceContext) -> String {
    if context.start_line == context.end_line {
        format!("{}:{}", context.path, context.start_line)
    } else {
        format!(
            "{}:{}-{}",
            context.path, context.start_line, context.end_line
        )
    }
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
            source_contexts: Vec::new(),
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
                clone_calls: 0,
                functions: Vec::new(),
                types: Vec::new(),
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
                clone_calls: 0,
                functions: Vec::new(),
                types: Vec::new(),
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
    fn human_report_explains_why_findings_matter_and_makes_repository_explorer_secondary() {
        use crate::model::{
            DeltaStatus, Evidence, EvidenceClass, Finding, FunctionFact, FunctionKind,
            ModuleMetrics, Priority,
        };

        let mut result = minimal_result();
        result.modules = vec![
            ModuleMetrics {
                crate_name: "demo".into(),
                module_path: "quiet".into(),
                path: "src/quiet.rs".into(),
                lines: 20,
                decision_sites: 2,
                public_items: 1,
                clone_calls: 0,
                functions: Vec::new(),
                types: Vec::new(),
                explicit_imports: Vec::new(),
                local_dependency_modules: Vec::new(),
                structure_digest: "quiet".into(),
                parse_complete: true,
                gate_complete: true,
                limitation: None,
                history: None,
            },
            ModuleMetrics {
                crate_name: "demo".into(),
                module_path: "engine".into(),
                path: "src/engine.rs".into(),
                lines: 120,
                decision_sites: 30,
                public_items: 4,
                clone_calls: 2,
                functions: vec![FunctionFact {
                    name: "run".into(),
                    kind: FunctionKind::Function,
                    public_declared: true,
                }],
                types: Vec::new(),
                explicit_imports: Vec::new(),
                local_dependency_modules: vec!["demo::quiet".into()],
                structure_digest: "engine".into(),
                parse_complete: true,
                gate_complete: true,
                limitation: None,
                history: None,
            },
        ];
        result.findings = vec![Finding {
            fingerprint: "engine".into(),
            rule: "structure.current_coupled_outlier".into(),
            subject: "demo::engine".into(),
            identity: "demo::engine".into(),
            configuration: "host".into(),
            evidence_class: EvidenceClass::Strong,
            priority: Priority::Investigate,
            delta: DeltaStatus::Current,
            gate: false,
            accepted: false,
            acceptance_reason: None,
            summary: "technical summary".into(),
            direction: "inspect boundary".into(),
            evidence: vec![
                Evidence {
                    metric: "decision_sites".into(),
                    value: 30,
                    reference: 12,
                    population: 20,
                    baseline: None,
                    material_delta: None,
                },
                Evidence {
                    metric: "local_dependency_modules".into(),
                    value: 8,
                    reference: 4,
                    population: 20,
                    baseline: None,
                    material_delta: None,
                },
            ],
        }];

        let rendered = html(&result);

        assert!(rendered.contains("What needs attention"));
        assert!(rendered.contains("This module may be harder to change safely"));
        assert!(rendered.contains("Why this matters"));
        assert!(rendered.contains("If ignored"));
        assert!(rendered.contains("What to investigate next"));
        assert!(rendered.contains("Repository explorer"));
        assert!(rendered.contains("Use this after a finding points you to an area"));
        assert!(rendered.contains("Worth investigating"));
        assert!(rendered.contains("No issue currently identified"));
        assert!(rendered.find("demo::engine").unwrap() < rendered.find("demo::quiet").unwrap());
        assert!(rendered.contains("Analysis details"));
        assert!(rendered.contains("Technical capability details"));
        assert!(!rendered.contains("<div class=\"card\"><h2>Coverage</h2>"));
    }

    #[test]
    fn compact_ai_json_keeps_actionable_evidence_without_raw_repository_inventory() {
        use crate::model::{
            DeltaStatus, Evidence, EvidenceClass, Finding, ModuleMetrics, Priority, SourceContext,
        };

        let mut result = minimal_result();
        result.modules = vec![ModuleMetrics {
            crate_name: "demo".into(),
            module_path: "engine".into(),
            path: "src/engine.rs".into(),
            lines: 50,
            decision_sites: 20,
            public_items: 2,
            clone_calls: 0,
            functions: Vec::new(),
            types: Vec::new(),
            explicit_imports: Vec::new(),
            local_dependency_modules: Vec::new(),
            structure_digest: "engine".into(),
            parse_complete: true,
            gate_complete: true,
            limitation: None,
            history: None,
        }];
        result.findings = vec![
            Finding {
                fingerprint: "active".into(),
                rule: "runtime.clone_syntax_outlier".into(),
                subject: "demo::engine".into(),
                identity: "demo::engine".into(),
                configuration: "host".into(),
                evidence_class: EvidenceClass::Candidate,
                priority: Priority::Observe,
                delta: DeltaStatus::Current,
                gate: false,
                accepted: false,
                acceptance_reason: None,
                summary: "raw summary".into(),
                direction: "measure".into(),
                evidence: vec![Evidence {
                    metric: "clone_call_syntax_sites".into(),
                    value: 12,
                    reference: 4,
                    population: 20,
                    baseline: None,
                    material_delta: None,
                }],
            },
            Finding {
                fingerprint: "accepted".into(),
                rule: "structure.current_coupled_outlier".into(),
                subject: "demo::engine".into(),
                identity: "demo::engine".into(),
                configuration: "host".into(),
                evidence_class: EvidenceClass::Strong,
                priority: Priority::Investigate,
                delta: DeltaStatus::Current,
                gate: false,
                accepted: true,
                acceptance_reason: Some("intentional".into()),
                summary: "accepted".into(),
                direction: "none".into(),
                evidence: Vec::new(),
            },
        ];
        result.source_contexts = vec![SourceContext {
            subject: "demo::engine".into(),
            metric: "clone_call_syntax_sites".into(),
            path: "src/engine.rs".into(),
            start_line: 12,
            end_line: 12,
            excerpt: "value.clone()".into(),
            excerpt_truncated: false,
        }];

        let first = super::ai_json(&result);
        let second = super::ai_json(&result);
        let value: serde_json::Value = serde_json::from_str(&first).unwrap();

        assert_eq!(first, second);
        assert_eq!(value["format"], "ferric_lens_ai");
        assert_eq!(value["findings"].as_array().unwrap().len(), 1);
        assert_eq!(
            value["findings"][0]["title"],
            "Repeated copying may be worth measuring"
        );
        assert_eq!(value["findings"][0]["path"], "src/engine.rs");
        assert_eq!(value["findings"][0]["source_contexts"][0]["start_line"], 12);
        assert!(value["findings"][0]["why_care"]
            .as_str()
            .unwrap()
            .contains("copy"));
        assert!(value.get("modules").is_none());
        assert!(value.get("capabilities").is_none());
        assert_eq!(value["summary"]["accepted_findings"], 1);
    }

    #[test]
    fn cli_summary_prioritizes_plain_language_actions_over_internal_rule_names() {
        use crate::model::{DeltaStatus, EvidenceClass, Finding, Priority};

        let mut result = minimal_result();
        result.findings = vec![Finding {
            fingerprint: "refactor".into(),
            rule: "refactor.multi_signal_candidate".into(),
            subject: "demo::engine".into(),
            identity: "demo::engine".into(),
            configuration: "host".into(),
            evidence_class: EvidenceClass::Strong,
            priority: Priority::Investigate,
            delta: DeltaStatus::Current,
            gate: false,
            accepted: false,
            acceptance_reason: None,
            summary: "internal summary".into(),
            direction: "inspect responsibilities".into(),
            evidence: Vec::new(),
        }];

        let rendered = super::cli_summary(&result);

        assert!(rendered.contains("1 area worth reviewing"));
        assert!(rendered.contains("Possible refactoring opportunity"));
        assert!(rendered.contains("Why it matters:"));
        assert!(rendered.contains("Next:"));
        assert!(!rendered.contains("refactor.multi_signal_candidate"));
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
                clone_calls: 0,
                functions: Vec::new(),
                types: Vec::new(),
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
                clone_calls: 0,
                functions: Vec::new(),
                types: Vec::new(),
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
    fn separates_structural_runtime_and_build_advisories() {
        use crate::model::{DeltaStatus, EvidenceClass, Finding, Priority};

        let mut result = minimal_result();
        for rule in [
            "refactor.multi_signal_candidate",
            "structure.current_coupled_outlier",
            "runtime.clone_syntax_outlier",
            "build.rebuild_exposure_candidate",
        ] {
            result.findings.push(Finding {
                fingerprint: rule.into(),
                rule: rule.into(),
                subject: "demo::m0".into(),
                identity: "demo::m0".into(),
                configuration: "host".into(),
                evidence_class: EvidenceClass::Candidate,
                priority: Priority::Observe,
                delta: DeltaStatus::Current,
                gate: false,
                accepted: false,
                acceptance_reason: None,
                summary: "candidate".into(),
                direction: "inspect".into(),
                evidence: Vec::new(),
            });
        }

        let rendered = html(&result);

        assert!(rendered.contains("<h2>Refactoring candidates</h2>"));
        assert!(rendered.contains("<h2>Structural advisories</h2>"));
        assert!(rendered.contains("<h2>Runtime-risk candidates</h2>"));
        assert!(rendered.contains("<h2>Build-efficiency candidates</h2>"));
    }

    #[test]
    fn codebase_map_renders_dependency_function_and_type_hierarchy() {
        use crate::model::{FunctionFact, FunctionKind, ModuleMetrics, TypeFact, TypeKind};

        let mut result = minimal_result();
        result.modules = vec![ModuleMetrics {
            crate_name: "demo".into(),
            module_path: "engine".into(),
            path: "src/engine.rs".into(),
            lines: 10,
            decision_sites: 1,
            public_items: 2,
            clone_calls: 1,
            functions: vec![
                FunctionFact {
                    name: "engine::run<&>".into(),
                    kind: FunctionKind::Function,
                    public_declared: true,
                },
                FunctionFact {
                    name: "engine::helper".into(),
                    kind: FunctionKind::Function,
                    public_declared: false,
                },
            ],
            types: vec![
                TypeFact {
                    name: "engine::State<&>".into(),
                    kind: TypeKind::Struct,
                    public_declared: false,
                },
                TypeFact {
                    name: "engine::PublicState".into(),
                    kind: TypeKind::Struct,
                    public_declared: true,
                },
            ],
            explicit_imports: Vec::new(),
            local_dependency_modules: vec!["demo::model<&>".into()],
            structure_digest: "engine".into(),
            parse_complete: true,
            gate_complete: true,
            limitation: None,
            history: None,
        }];

        let rendered = html(&result);

        assert!(rendered.contains("<summary>Dependencies (1)</summary>"));
        assert!(rendered.contains("demo::model&lt;&amp;&gt;"));
        assert!(rendered.contains("<summary>Functions (2)</summary>"));
        assert!(rendered.contains("engine::run&lt;&amp;&gt;"));
        assert!(rendered.contains("function · public"));
        assert!(rendered.contains("engine::helper"));
        assert!(rendered.contains("function · private"));
        assert!(rendered.contains("<summary>Types (2)</summary>"));
        assert!(rendered.contains("engine::State&lt;&amp;&gt;"));
        assert!(rendered.contains("struct · private"));
        assert!(rendered.contains("engine::PublicState"));
        assert!(rendered.contains("struct · public"));
    }

    #[test]
    fn report_exposes_human_triage_labels_summary_and_module_navigation() {
        use crate::model::{DeltaStatus, EvidenceClass, Finding, ModuleMetrics, Priority};

        let mut result = minimal_result();
        result.modules = vec![ModuleMetrics {
            crate_name: "demo".into(),
            module_path: "engine".into(),
            path: "src/engine.rs".into(),
            lines: 42,
            decision_sites: 9,
            public_items: 1,
            clone_calls: 0,
            functions: Vec::new(),
            types: Vec::new(),
            explicit_imports: Vec::new(),
            local_dependency_modules: Vec::new(),
            structure_digest: "engine".into(),
            parse_complete: true,
            gate_complete: true,
            limitation: None,
            history: None,
        }];
        result.findings = vec![
            Finding {
                fingerprint: "refactor".into(),
                rule: "refactor.multi_signal_candidate".into(),
                subject: "demo::engine".into(),
                identity: "demo::engine".into(),
                configuration: "host".into(),
                evidence_class: EvidenceClass::Strong,
                priority: Priority::Investigate,
                delta: DeltaStatus::Worsened,
                gate: false,
                accepted: false,
                acceptance_reason: None,
                summary: "corroborated".into(),
                direction: "inspect boundary".into(),
                evidence: Vec::new(),
            },
            Finding {
                fingerprint: "observe".into(),
                rule: "runtime.clone_syntax_outlier".into(),
                subject: "demo::engine".into(),
                identity: "demo::engine".into(),
                configuration: "host".into(),
                evidence_class: EvidenceClass::Candidate,
                priority: Priority::Observe,
                delta: DeltaStatus::Current,
                gate: false,
                accepted: false,
                acceptance_reason: None,
                summary: "observe".into(),
                direction: "measure".into(),
                evidence: Vec::new(),
            },
        ];

        let rendered = html(&result);

        assert!(rendered.contains("<h2>Triage</h2>"));
        assert!(rendered.contains("1 investigate"));
        assert!(rendered.contains("1 observe"));
        assert!(rendered.contains("1 refactoring candidate"));
        assert!(rendered.contains("1 new/worsened finding"));
        assert!(rendered.contains("Priority 2 — investigate"));
        assert!(rendered.contains("strong evidence"));
        assert!(rendered.contains("worsened"));
        assert!(rendered.contains("src/engine.rs"));
        assert!(rendered.contains(">View module</a>"));
        assert!(rendered.contains("id=\"module-"));
        assert!(rendered.contains("href=\"#module-"));
    }

    #[test]
    fn human_report_orders_priority_then_change_relevance_without_mutating_result() {
        use crate::model::{DeltaStatus, EvidenceClass, Finding, Priority};

        let mut result = minimal_result();
        for (fingerprint, subject, priority, delta) in [
            (
                "observe",
                "demo::observe",
                Priority::Observe,
                DeltaStatus::Current,
            ),
            (
                "investigate-current",
                "demo::investigate-current",
                Priority::Investigate,
                DeltaStatus::Current,
            ),
            (
                "investigate-worsened",
                "demo::investigate-worsened",
                Priority::Investigate,
                DeltaStatus::Worsened,
            ),
            ("act", "demo::act", Priority::ActFirst, DeltaStatus::New),
        ] {
            result.findings.push(Finding {
                fingerprint: fingerprint.into(),
                rule: "structure.test".into(),
                subject: subject.into(),
                identity: subject.into(),
                configuration: "host".into(),
                evidence_class: EvidenceClass::Candidate,
                priority,
                delta,
                gate: false,
                accepted: false,
                acceptance_reason: None,
                summary: "candidate".into(),
                direction: "inspect".into(),
                evidence: Vec::new(),
            });
        }
        let original = result.findings.clone();

        let rendered = html(&result);

        let act = rendered.find("demo::act").unwrap();
        let worsened = rendered.find("demo::investigate-worsened").unwrap();
        let investigate = rendered.find("demo::investigate-current").unwrap();
        let observe = rendered.find("demo::observe").unwrap();
        assert!(act < worsened);
        assert!(worsened < investigate);
        assert!(investigate < observe);
        assert_eq!(result.findings, original);
    }

    #[test]
    fn human_report_labels_cover_all_model_states() {
        use crate::model::{DeltaStatus, EvidenceClass, Priority};

        assert_eq!(
            super::priority_label(&Priority::ActFirst),
            "Priority 1 — act first"
        );
        assert_eq!(
            super::priority_label(&Priority::Investigate),
            "Priority 2 — investigate"
        );
        assert_eq!(super::priority_label(&Priority::Observe), "Observe");
        assert_eq!(
            super::evidence_label(&EvidenceClass::Proven),
            "proven evidence"
        );
        assert_eq!(
            super::evidence_label(&EvidenceClass::Strong),
            "strong evidence"
        );
        assert_eq!(
            super::evidence_label(&EvidenceClass::Candidate),
            "candidate evidence"
        );
        assert_eq!(super::delta_label(&DeltaStatus::Current), "current");
        assert_eq!(super::delta_label(&DeltaStatus::New), "new");
        assert_eq!(super::delta_label(&DeltaStatus::Worsened), "worsened");
        assert_eq!(super::delta_label(&DeltaStatus::Unchanged), "unchanged");
        assert_eq!(super::delta_label(&DeltaStatus::Unknown), "unknown");
        assert_eq!(super::count_phrase(2, "item", "items"), "2 items");
    }

    #[test]
    fn human_report_ordering_covers_stable_tiebreakers_and_ranks() {
        use crate::model::{DeltaStatus, EvidenceClass, Finding, Priority};

        assert_eq!(super::priority_rank(&Priority::ActFirst), 0);
        assert_eq!(super::priority_rank(&Priority::Investigate), 1);
        assert_eq!(super::priority_rank(&Priority::Observe), 2);
        assert_eq!(super::delta_rank(&DeltaStatus::Worsened), 0);
        assert_eq!(super::delta_rank(&DeltaStatus::New), 1);
        assert_eq!(super::delta_rank(&DeltaStatus::Current), 2);
        assert_eq!(super::delta_rank(&DeltaStatus::Unchanged), 3);
        assert_eq!(super::delta_rank(&DeltaStatus::Unknown), 4);

        let mut result = minimal_result();
        for (fingerprint, rule, subject) in [
            ("b", "structure.z", "demo::b"),
            ("a-z", "structure.z", "demo::a"),
            ("a-a", "structure.a", "demo::a"),
        ] {
            result.findings.push(Finding {
                fingerprint: fingerprint.into(),
                rule: rule.into(),
                subject: subject.into(),
                identity: subject.into(),
                configuration: "host".into(),
                evidence_class: EvidenceClass::Candidate,
                priority: Priority::Observe,
                delta: DeltaStatus::Current,
                gate: false,
                accepted: false,
                acceptance_reason: None,
                summary: "candidate".into(),
                direction: "inspect".into(),
                evidence: Vec::new(),
            });
        }

        let rendered = html(&result);

        assert!(rendered.find("structure.a").unwrap() < rendered.find("structure.z").unwrap());
        assert!(rendered.find("demo::a").unwrap() < rendered.find("demo::b").unwrap());
    }

    #[test]
    fn source_evidence_renders_ranges_truncation_and_escaped_excerpt() {
        use crate::model::{
            DeltaStatus, Evidence, EvidenceClass, Finding, Priority, SourceContext,
        };

        let mut result = minimal_result();
        result.findings = vec![Finding {
            fingerprint: "source-context".into(),
            rule: "structure.test".into(),
            subject: "demo::engine".into(),
            identity: "demo::engine".into(),
            configuration: "host".into(),
            evidence_class: EvidenceClass::Strong,
            priority: Priority::Investigate,
            delta: DeltaStatus::Current,
            gate: false,
            accepted: false,
            acceptance_reason: None,
            summary: "source context".into(),
            direction: "inspect".into(),
            evidence: vec![Evidence {
                metric: "decision_sites".into(),
                value: 2,
                reference: 1,
                population: 20,
                baseline: None,
                material_delta: None,
            }],
        }];
        result.source_contexts = vec![SourceContext {
            subject: "demo::engine".into(),
            metric: "decision_sites".into(),
            path: "src/engine.rs".into(),
            start_line: 7,
            end_line: 9,
            excerpt: "<unsafe & excerpt>".into(),
            excerpt_truncated: true,
        }];

        let rendered = html(&result);

        assert!(rendered.contains("src/engine.rs:7-9"));
        assert!(rendered.contains("&lt;unsafe &amp; excerpt&gt;"));
        assert!(rendered.contains("Excerpt bounded for report size"));
        assert_eq!(
            super::source_location_label(&result.source_contexts[0]),
            "src/engine.rs:7-9"
        );

        result.source_contexts[0].end_line = 7;
        assert_eq!(
            super::source_location_label(&result.source_contexts[0]),
            "src/engine.rs:7"
        );
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
