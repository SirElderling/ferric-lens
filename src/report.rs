use std::{
    fs,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

use serde::Serialize;

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

#[derive(Debug, Clone, Copy)]
struct FindingGuidance {
    title: &'static str,
    why_care: &'static str,
    if_ignored: &'static str,
}

#[derive(Serialize)]
struct AiSummary {
    areas_worth_reviewing: usize,
    act_first: usize,
    investigate: usize,
    observe: usize,
    blocking_findings: usize,
    accepted_findings: usize,
}

#[derive(Serialize)]
struct AiLimit<'a> {
    capability: &'a str,
    status: &'static str,
    detail: Option<&'a str>,
}

#[derive(Serialize)]
struct AiFinding<'a> {
    priority: &'static str,
    evidence_strength: &'static str,
    delta: &'static str,
    gate: bool,
    title: &'static str,
    subject: &'a str,
    path: String,
    why_care: &'static str,
    next_step: &'a str,
    rule: &'a str,
    evidence: &'a [crate::model::Evidence],
    source_contexts: Vec<&'a SourceContext>,
}

#[derive(Serialize)]
struct AiOutput<'a> {
    format: &'static str,
    schema_version: u32,
    result_digest: String,
    verdict: &'static str,
    verdict_reason: &'a str,
    summary: AiSummary,
    findings: Vec<AiFinding<'a>>,
    analysis_limits: Vec<AiLimit<'a>>,
}

pub fn ai_json(result: &AnalysisResult) -> String {
    let active = ordered_findings(
        result
            .findings
            .iter()
            .filter(|finding| !finding.accepted)
            .collect(),
    );
    let findings = active
        .iter()
        .map(|finding| {
            let guidance = finding_guidance(finding);
            AiFinding {
                priority: priority_label(&finding.priority),
                evidence_strength: evidence_label(&finding.evidence_class),
                delta: delta_label(&finding.delta),
                gate: finding.gate,
                title: guidance.title,
                subject: &finding.subject,
                path: finding_path(finding, &result.modules, &result.source_contexts),
                why_care: guidance.why_care,
                next_step: &finding.direction,
                rule: &finding.rule,
                evidence: &finding.evidence,
                source_contexts: contexts_for_finding(finding, &result.source_contexts),
            }
        })
        .collect::<Vec<_>>();

    let output = AiOutput {
        format: "ferric_lens_ai",
        schema_version: 1,
        result_digest: result_digest(result),
        verdict: verdict_label(&result.verdict),
        verdict_reason: &result.verdict_reason,
        summary: ai_summary(result),
        findings,
        analysis_limits: result
            .capabilities
            .iter()
            .filter(|capability| capability.status != crate::model::CapabilityStatus::Complete)
            .map(|capability| AiLimit {
                capability: &capability.name,
                status: capability_status_label(&capability.status),
                detail: capability.detail.as_deref(),
            })
            .collect(),
    };

    serde_json::to_string_pretty(&output).expect("AI output is JSON-serializable")
}

pub fn cli_summary(result: &AnalysisResult) -> String {
    let active = ordered_findings(
        result
            .findings
            .iter()
            .filter(|finding| !finding.accepted)
            .collect(),
    );
    let summary = ai_summary(result);
    let mut output = String::new();
    output.push_str(&format!(
        "Ferric Lens: {}\n{}\n",
        verdict_label(&result.verdict),
        result.verdict_reason
    ));
    if let Some(baseline) = &result.baseline {
        output.push_str(&format!(
            "Baseline: {} (merge base {})\n",
            baseline.target_ref, baseline.merge_base
        ));
    }
    output.push_str(&format!(
        "{}\n",
        count_phrase(
            summary.areas_worth_reviewing,
            "area worth reviewing",
            "areas worth reviewing"
        )
    ));

    if active.is_empty() {
        output.push_str(
            "No active finding currently has enough evidence to recommend investigation.\n",
        );
    } else {
        for finding in active.iter().take(5) {
            let guidance = finding_guidance(finding);
            let path = finding_path(finding, &result.modules, &result.source_contexts);
            output.push_str(&format!(
                "\n[{}] {} — {}\nWhy it matters: {}\nNext: {}\n",
                priority_label(&finding.priority),
                guidance.title,
                path,
                guidance.why_care,
                finding.direction
            ));
        }
        if active.len() > 5 {
            output.push_str(&format!(
                "\n{} additional active finding(s) are available in the HTML/full JSON report.\n",
                active.len() - 5
            ));
        }
    }

    let limits = result
        .capabilities
        .iter()
        .filter(|capability| capability.status != crate::model::CapabilityStatus::Complete)
        .count();
    if limits > 0 {
        output.push_str(&format!(
            "\nAnalysis note: {limits} capability limitation(s) were recorded; see the HTML analysis details or canonical JSON for technical detail.\n"
        ));
    }
    output
}

fn ai_summary(result: &AnalysisResult) -> AiSummary {
    let active = result
        .findings
        .iter()
        .filter(|finding| !finding.accepted)
        .collect::<Vec<_>>();
    AiSummary {
        areas_worth_reviewing: active
            .iter()
            .map(|finding| finding.subject.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        act_first: active
            .iter()
            .filter(|finding| finding.priority == Priority::ActFirst)
            .count(),
        investigate: active
            .iter()
            .filter(|finding| finding.priority == Priority::Investigate)
            .count(),
        observe: active
            .iter()
            .filter(|finding| finding.priority == Priority::Observe)
            .count(),
        blocking_findings: active.iter().filter(|finding| finding.gate).count(),
        accepted_findings: result
            .findings
            .iter()
            .filter(|finding| finding.accepted)
            .count(),
    }
}

fn ordered_findings(mut findings: Vec<&Finding>) -> Vec<&Finding> {
    findings.sort_by(|a, b| {
        priority_rank(&a.priority)
            .cmp(&priority_rank(&b.priority))
            .then_with(|| delta_rank(&a.delta).cmp(&delta_rank(&b.delta)))
            .then_with(|| a.subject.cmp(&b.subject))
            .then_with(|| a.rule.cmp(&b.rule))
    });
    findings
}

fn finding_guidance(finding: &Finding) -> FindingGuidance {
    match finding.rule.as_str() {
        "structure.coupled_complexity_growth" | "structure.current_coupled_outlier" => {
            FindingGuidance {
                title: "This module may be harder to change safely",
                why_care: "It combines unusually complex control flow with a broad repository dependency surface. Changes here can require understanding more execution paths and more neighboring modules at the same time.",
                if_ignored: "If responsibilities continue to accumulate, routine changes can become slower to review and test and can carry a larger risk of unintended side effects.",
            }
        }
        "structure.small_population_decision_concentration" => FindingGuidance {
            title: "Complex logic is concentrated here",
            why_care: "A disproportionate amount of branching is concentrated in this module relative to the small comparable population. That can make behavior harder to reason about even when the repository is too small for a stronger outlier claim.",
            if_ignored: "Additional branching can make future behavior changes harder to understand and test, especially if unrelated responsibilities collect in the same area.",
        },
        "runtime.clone_syntax_outlier" => FindingGuidance {
            title: "Repeated copying may be worth measuring",
            why_care: "This module contains unusually many observed .clone() calls. Some clones are cheap and intentional; others copy owned data, so this is a prompt to inspect what is being copied and how often the code runs.",
            if_ignored: "If the cloned values are large or this path executes frequently, unnecessary copying can consume memory bandwidth or allocation work. Ferric Lens has not established that this is happening.",
        },
        "build.rebuild_exposure_candidate" => FindingGuidance {
            title: "Changes here may affect many parts of the repository",
            why_care: "Many repository modules depend on this area. A frequently changing shared boundary can increase the amount of code that must be reconsidered or rebuilt after a change.",
            if_ignored: "A broad, unstable dependency boundary can gradually increase change coordination and incremental-build cost. This finding is structural evidence, not a measured compile-time claim.",
        },
        "refactor.multi_signal_candidate" => FindingGuidance {
            title: "Possible refactoring opportunity",
            why_care: "Multiple independent signals point to the same module. Corroborating evidence makes it more useful to inspect than a module flagged by only one isolated metric.",
            if_ignored: "If the signals reflect accumulating responsibilities, the module can become progressively harder to understand, change, and isolate. Ferric Lens does not prescribe a final architecture.",
        },
        _ => FindingGuidance {
            title: "This area is worth reviewing",
            why_care: "Ferric Lens found deterministic evidence that meets the rule's investigation threshold. Review the evidence in context before deciding whether a change is needed.",
            if_ignored: "The practical impact depends on the code and workload. Treat this as a focused investigation prompt rather than proof that the design is wrong.",
        },
    }
}

fn finding_path(
    finding: &Finding,
    modules: &[ModuleMetrics],
    source_contexts: &[SourceContext],
) -> String {
    modules
        .iter()
        .find(|module| module_subject(module) == finding.subject)
        .map(|module| module.path.clone())
        .or_else(|| {
            source_contexts
                .iter()
                .find(|context| context.subject == finding.subject)
                .map(|context| context.path.clone())
        })
        .unwrap_or_else(|| finding.subject.clone())
}

fn contexts_for_finding<'a>(
    finding: &Finding,
    source_contexts: &'a [SourceContext],
) -> Vec<&'a SourceContext> {
    let evidence_metrics = finding
        .evidence
        .iter()
        .map(|evidence| evidence.metric.as_str())
        .collect::<Vec<_>>();
    source_contexts
        .iter()
        .filter(|context| {
            context.subject == finding.subject
                && evidence_metrics.contains(&context.metric.as_str())
        })
        .collect()
}

fn capability_status_label(status: &crate::model::CapabilityStatus) -> &'static str {
    match status {
        crate::model::CapabilityStatus::Complete => "complete",
        crate::model::CapabilityStatus::Partial => "partial",
        crate::model::CapabilityStatus::Unavailable => "unavailable",
    }
}

fn verdict_label(verdict: &GateVerdict) -> &'static str {
    match verdict {
        GateVerdict::Pass => "PASS",
        GateVerdict::Regression => "REGRESSION",
        GateVerdict::Inconclusive => "INCONCLUSIVE",
    }
}

pub fn html(result: &AnalysisResult) -> String {
    let result_digest = result_digest(result);
    let active_findings = ordered_findings(
        result
            .findings
            .iter()
            .filter(|finding| !finding.accepted)
            .collect(),
    );
    let accepted_findings = ordered_findings(
        result
            .findings
            .iter()
            .filter(|finding| finding.accepted)
            .collect(),
    );
    let active_html = render_findings(
        &active_findings,
        "No active finding currently has enough evidence to recommend investigation.",
        &result.modules,
        &result.source_contexts,
    );
    let accepted_html = render_findings(
        &accepted_findings,
        "No findings have been explicitly accepted.",
        &result.modules,
        &result.source_contexts,
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
* {{ box-sizing: border-box; }}
body {{ max-width: 1180px; margin: 0 auto; padding: 2rem; line-height: 1.55; }}
header {{ border-bottom: 1px solid color-mix(in srgb, currentColor 20%, transparent); margin-bottom: 2rem; padding-bottom: 1rem; }}
h1, h2, h3 {{ line-height: 1.2; }}
.verdict {{ font-size: 1.35rem; font-weight: 750; margin-bottom: .35rem; }}
.muted, .finding-meta, .location {{ opacity: .78; }}
.attention-grid, .overview-grid {{ display: grid; grid-template-columns: repeat(auto-fit,minmax(260px,1fr)); gap: 1rem; }}
.card, article, details {{ min-width: 0; overflow-wrap: anywhere; }}
.card, article.finding-card, .analysis-details, .explorer-module {{ border: 1px solid color-mix(in srgb, currentColor 20%, transparent); border-radius: .7rem; padding: 1rem; margin: .8rem 0; }}
article.finding-card.gate {{ border-width: 2px; }}
.finding-header {{ display: flex; justify-content: space-between; align-items: flex-start; gap: 1rem; flex-wrap: wrap; }}
.badges {{ display: flex; flex-wrap: wrap; gap: .4rem; }}
.badge {{ border: 1px solid color-mix(in srgb, currentColor 25%, transparent); border-radius: 999px; padding: .15rem .55rem; font-size: .82rem; }}
.guidance {{ display: grid; grid-template-columns: repeat(auto-fit,minmax(230px,1fr)); gap: .8rem; margin: 1rem 0; }}
.guidance > div {{ border-left: 3px solid color-mix(in srgb, currentColor 25%, transparent); padding-left: .8rem; }}
.guidance h4 {{ margin: 0 0 .3rem; }}
summary {{ cursor: pointer; }}
.source-evidence {{ margin: .8rem 0; }}
.source-context {{ margin: .7rem 0; }}
.source-context pre {{ margin: .35rem 0; padding: .7rem; overflow-x: auto; white-space: pre-wrap; background: color-mix(in srgb, currentColor 6%, transparent); border-radius: .4rem; }}
.metric-list {{ margin: .5rem 0; padding-left: 1.2rem; }}
.metric-list li {{ margin: .25rem 0; }}
.explorer-intro {{ max-width: 760px; }}
.explorer-summary {{ display: flex; justify-content: space-between; align-items: baseline; gap: 1rem; flex-wrap: wrap; }}
.explorer-status {{ font-weight: 650; }}
.explorer-metrics {{ display: flex; flex-wrap: wrap; gap: .5rem 1rem; margin: .8rem 0; }}
.explorer-metrics span {{ white-space: nowrap; }}
.explorer-detail {{ margin-top: .8rem; }}
.analysis-details .detail-grid {{ display: grid; grid-template-columns: repeat(auto-fit,minmax(250px,1fr)); gap: 1rem; }}
.technical-list {{ padding-left: 1.2rem; }}
.technical-list li {{ margin: .35rem 0; overflow-wrap: anywhere; }}
.digest {{ word-break: break-all; }}
code {{ overflow-wrap: anywhere; }}
a {{ color: inherit; }}
@media (max-width: 650px) {{
  body {{ padding: 1rem; }}
  .guidance, .attention-grid, .overview-grid {{ grid-template-columns: 1fr; }}
}}
</style>
</head>
<body>
<header>
<h1>Ferric Lens</h1>
<p class="verdict">Analysis result: {verdict}</p>
<p>{reason}</p>
</header>

<section>
<h2>What needs attention</h2>
<div class="attention-grid"><div class="card">{triage}</div></div>
{active_html}
</section>

<section>
<h2>Repository overview</h2>
{overview}
</section>

<section>
<h2>Repository explorer</h2>
<p class="explorer-intro">Use this after a finding points you to an area, or when you want structural context for a module. Metrics here are context, not problems by themselves.</p>
{explorer}
</section>

<details class="analysis-details">
<summary><strong>Analysis details</strong> — baseline, configuration, history, capabilities, and provenance</summary>
{details}
</details>

<details class="analysis-details">
<summary><strong>Accepted findings</strong> ({accepted_count})</summary>
{accepted_html}
</details>
</body></html>"#,
        verdict = verdict_label(&result.verdict),
        reason = escape(&result.verdict_reason),
        triage = render_triage_summary(result),
        overview = render_repository_overview(result),
        explorer = render_repository_explorer(result),
        details = render_analysis_details(result, &result_digest),
        accepted_count = accepted_findings.len(),
    )
}

fn render_repository_overview(result: &AnalysisResult) -> String {
    let subjects = active_subjects(result);
    let partial_capabilities = result
        .capabilities
        .iter()
        .filter(|capability| capability.status != crate::model::CapabilityStatus::Complete)
        .count();

    let quality = if result.architecture.incomplete_modules == 0 && partial_capabilities == 0 {
        "Ferric Lens completed the available evidence checks for this run. Metrics without findings are context only.".to_owned()
    } else {
        format!(
            "Some evidence is incomplete: {} of {} modules have incomplete graph evidence and {} capability limitation(s) were recorded. Missing evidence is not treated as proof that an area is healthy.",
            result.architecture.incomplete_modules,
            result.architecture.modules,
            partial_capabilities
        )
    };
    let cycle_text = if result.architecture.cycles.is_empty() {
        "No explicit-import dependency cycles were observed.".to_owned()
    } else {
        format!(
            "{} explicit-import dependency cycle(s) were observed and may deserve architectural review.",
            result.architecture.cycles.len()
        )
    };

    format!(
        r#"<div class="overview-grid">
<div class="card"><h3>Attention</h3><p><strong>{areas}</strong></p><p class="muted">Only areas with enough evidence to justify investigation are counted here.</p></div>
<div class="card"><h3>Structure</h3><p><strong>{modules} modules</strong><br>{edges} resolved repository dependency edges</p><p>{cycles}</p></div>
<div class="card"><h3>Analysis confidence</h3><p>{quality}</p></div>
</div>"#,
        areas = count_phrase(
            subjects.len(),
            "area worth reviewing",
            "areas worth reviewing",
        ),
        modules = result.architecture.modules,
        edges = result.architecture.explicit_dependency_edges,
        cycles = escape(&cycle_text),
        quality = escape(&quality),
    )
}

fn render_repository_explorer(result: &AnalysisResult) -> String {
    let active = result
        .findings
        .iter()
        .filter(|finding| !finding.accepted)
        .collect::<Vec<_>>();
    let mut modules = result.modules.iter().collect::<Vec<_>>();
    modules.sort_by(|a, b| {
        let a_subject = module_subject(a);
        let b_subject = module_subject(b);
        let a_findings = active
            .iter()
            .filter(|finding| finding.subject == a_subject)
            .copied()
            .collect::<Vec<_>>();
        let b_findings = active
            .iter()
            .filter(|finding| finding.subject == b_subject)
            .copied()
            .collect::<Vec<_>>();
        best_priority_rank(&a_findings)
            .cmp(&best_priority_rank(&b_findings))
            .then_with(|| b_findings.len().cmp(&a_findings.len()))
            .then_with(|| a.path.cmp(&b.path))
    });

    let mut html = String::new();
    for module in modules {
        let subject = module_subject(module);
        let module_findings = active
            .iter()
            .filter(|finding| finding.subject == subject)
            .copied()
            .collect::<Vec<_>>();
        let anchor = anchor_id("module", &subject);
        let status = if module_findings.is_empty() {
            "No issue currently identified".to_owned()
        } else {
            format!(
                "Worth investigating — {}",
                count_phrase(module_findings.len(), "finding", "findings")
            )
        };

        html.push_str(r#"<details class="explorer-module" id=""#);
        html.push_str(&anchor);
        html.push_str(r#""><summary><span class="explorer-summary"><span><code>"#);
        html.push_str(&escape(&module.path));
        html.push_str("</code><br><small>");
        html.push_str(&escape(&subject));
        html.push_str(r#"</small></span><span class="explorer-status">"#);
        html.push_str(&escape(&status));
        html.push_str("</span></span></summary>");

        if module_findings.is_empty() {
            html.push_str("<p>These metrics are shown for context. Ferric Lens did not find enough evidence to recommend investigating this module.</p>");
        } else {
            html.push_str("<p>Start with the finding");
            if module_findings.len() != 1 {
                html.push('s');
            }
            html.push_str(
                " above. This explorer shows structural context for the affected area.</p><p>",
            );
            for (index, finding) in module_findings.iter().enumerate() {
                if index > 0 {
                    html.push_str(" · ");
                }
                html.push_str(r##"<a href="#"##);
                html.push_str(&anchor_id("finding", &finding.fingerprint));
                html.push_str(r#"">View finding</a>"#);
            }
            html.push_str("</p>");
        }

        html.push_str(r#"<div class="explorer-metrics">"#);
        html.push_str(&format!(
            "<span><strong>{}</strong> decision points</span><span><strong>{}</strong> repository dependencies</span><span><strong>{}</strong> public items</span><span><strong>{}</strong> clone sites</span><span><strong>{}</strong> lines</span>",
            module.decision_sites,
            module.local_dependency_modules.len(),
            module.public_items,
            module.clone_calls,
            module.lines
        ));
        html.push_str("</div>");

        if !module.gate_complete {
            html.push_str(r#"<p class="muted">Some gate-relevant evidence for this module is incomplete.</p>"#);
        }
        if !module.local_dependency_modules.is_empty() {
            html.push_str(r#"<details class="explorer-detail"><summary>Dependencies ("#);
            html.push_str(&module.local_dependency_modules.len().to_string());
            html.push_str(")</summary><ul>");
            for dependency in &module.local_dependency_modules {
                html.push_str("<li><code>");
                html.push_str(&escape(dependency));
                html.push_str("</code></li>");
            }
            html.push_str("</ul></details>");
        }
        if !module.functions.is_empty() {
            html.push_str(r#"<details class="explorer-detail"><summary>Functions ("#);
            html.push_str(&module.functions.len().to_string());
            html.push_str(")</summary><ul>");
            for function in &module.functions {
                html.push_str("<li><code>");
                html.push_str(&escape(&function.name));
                html.push_str("</code> · ");
                html.push_str(&escape(&format!("{:?}", function.kind).to_lowercase()));
                html.push_str(" · ");
                html.push_str(if function.public_declared {
                    "public"
                } else {
                    "private"
                });
                html.push_str("</li>");
            }
            html.push_str("</ul></details>");
        }
        if !module.types.is_empty() {
            html.push_str(r#"<details class="explorer-detail"><summary>Types ("#);
            html.push_str(&module.types.len().to_string());
            html.push_str(")</summary><ul>");
            for item_type in &module.types {
                html.push_str("<li><code>");
                html.push_str(&escape(&item_type.name));
                html.push_str("</code> · ");
                html.push_str(&escape(&format!("{:?}", item_type.kind).to_lowercase()));
                html.push_str(" · ");
                html.push_str(if item_type.public_declared {
                    "public"
                } else {
                    "private"
                });
                html.push_str("</li>");
            }
            html.push_str("</ul></details>");
        }
        if let Some(history) = &module.history {
            html.push_str(
                r#"<details class="explorer-detail"><summary>History context</summary><p>"#,
            );
            html.push_str(&escape(&format!(
                "Changed in {} of {} sampled non-merge commits.",
                history.change_commits, history.sampled_commits
            )));
            if !history.cochange.is_empty() {
                html.push_str("</p><p>Often changed with: ");
                for (index, related) in history.cochange.iter().enumerate() {
                    if index > 0 {
                        html.push_str(", ");
                    }
                    html.push_str("<code>");
                    html.push_str(&escape(&related.path));
                    html.push_str("</code>");
                }
            }
            html.push_str("</p></details>");
        }
        html.push_str("</details>");
    }
    html
}

fn render_analysis_details(result: &AnalysisResult, digest: &str) -> String {
    let baseline = result.baseline.as_ref().map_or_else(
        || "<p>No comparable baseline was available.</p>".to_owned(),
        |baseline| {
            format!(
                r#"<p><strong>Target:</strong> <code>{}</code><br><strong>Merge base:</strong> <code class="digest">{}</code><br>{} baseline source files</p>"#,
                escape(&baseline.target_ref),
                escape(&baseline.merge_base),
                baseline.source_files
            )
        },
    );
    let profile = format!(
        "<p><strong>{}</strong><br>resolved target <code>{}</code><br>{}</p>",
        escape(&result.profile.id),
        escape(&result.profile.resolved_target),
        if result.profile.features.is_empty() {
            "default features only".to_owned()
        } else {
            format!(
                "default + explicit features <code>{}</code>",
                escape(&result.profile.features.join(","))
            )
        }
    );
    let history = result.history.as_ref().map_or_else(
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
    let external = result.imported_evidence.as_ref().map_or_else(
        || "<p>No external evidence imported.</p>".to_owned(),
        |evidence| {
            let mut html = format!(
                "<p><strong>{}</strong> {}<br>{}<br>{}</p>",
                escape(&evidence.producer),
                escape(&evidence.producer_version),
                if evidence.attached {
                    "attached to current analysis"
                } else {
                    "unattached context only"
                },
                escape(&evidence.attachment_reason)
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

    let mut capabilities = String::new();
    for capability in &result.capabilities {
        capabilities.push_str("<li><strong>");
        capabilities.push_str(&escape(&capability.name));
        capabilities.push_str("</strong>: ");
        capabilities.push_str(capability_status_label(&capability.status));
        if let Some(detail) = &capability.detail {
            capabilities.push_str(" — ");
            capabilities.push_str(&escape(detail));
        }
        capabilities.push_str("</li>");
    }

    format!(
        r#"<div class="detail-grid">
<div><h3>Baseline</h3>{baseline}</div>
<div><h3>Analysis profile</h3>{profile}</div>
<div><h3>History</h3>{history}</div>
<div><h3>External evidence</h3>{external}</div>
<div><h3>Provenance</h3><p><strong>Result digest</strong><br><code class="digest">{result_digest}</code></p><p><strong>Source digest</strong><br><code class="digest">{source_digest}</code></p></div>
</div>
<details><summary><strong>Technical capability details</strong> ({capability_count})</summary><ul class="technical-list">{capabilities}</ul></details>"#,
        result_digest = escape(digest),
        source_digest = escape(&result.snapshot.content_digest),
        capability_count = result.capabilities.len(),
    )
}

fn render_triage_summary(result: &AnalysisResult) -> String {
    let summary = ai_summary(result);
    format!(
        r#"<h3>{}</h3><p>{} · {} · {}</p><p class="muted">Ferric Lens only recommends investigation when deterministic evidence meets a rule threshold. Large metric values alone are not treated as problems.</p>"#,
        count_phrase(
            summary.areas_worth_reviewing,
            "area worth reviewing",
            "areas worth reviewing"
        ),
        count_phrase(summary.act_first, "act first", "act first"),
        count_phrase(summary.investigate, "investigate", "investigate"),
        count_phrase(summary.observe, "observe", "observe"),
    )
}

fn active_subjects(result: &AnalysisResult) -> std::collections::BTreeSet<&str> {
    result
        .findings
        .iter()
        .filter(|finding| !finding.accepted)
        .map(|finding| finding.subject.as_str())
        .collect()
}

fn best_priority_rank(findings: &[&Finding]) -> u8 {
    findings
        .iter()
        .map(|finding| priority_rank(&finding.priority))
        .min()
        .unwrap_or(u8::MAX)
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

    let ordered = ordered_findings(findings.to_vec());
    let mut html = String::new();
    for finding in ordered {
        let guidance = finding_guidance(finding);
        let path = finding_path(finding, modules, source_contexts);
        let finding_anchor = anchor_id("finding", &finding.fingerprint);

        html.push_str(if finding.gate {
            r#"<article class="finding-card gate" id=""#
        } else {
            r#"<article class="finding-card" id=""#
        });
        html.push_str(&finding_anchor);
        html.push_str(r#""><div class="finding-header"><div><h3>"#);
        html.push_str(&escape(guidance.title));
        html.push_str("</h3><p><code>");
        html.push_str(&escape(&path));
        html.push_str("</code><br><small>");
        html.push_str(&escape(&finding.subject));
        html.push_str(r#"</small></p></div><div class="badges"><span class="badge">"#);
        html.push_str(priority_label(&finding.priority));
        html.push_str(r#"</span><span class="badge">"#);
        html.push_str(evidence_label(&finding.evidence_class));
        html.push_str(r#"</span><span class="badge">"#);
        html.push_str(delta_label(&finding.delta));
        html.push_str("</span>");
        if finding.gate {
            html.push_str(r#"<span class="badge">CI gate</span>"#);
        }
        if finding.accepted {
            html.push_str(r#"<span class="badge">accepted</span>"#);
        }
        html.push_str("</div></div>");

        html.push_str(r#"<div class="guidance"><div><h4>Why this matters</h4><p>"#);
        html.push_str(&escape(guidance.why_care));
        html.push_str(r#"</p></div><div><h4>If ignored</h4><p>"#);
        html.push_str(&escape(guidance.if_ignored));
        html.push_str("</p></div></div>");

        html.push_str("<h4>What Ferric Lens found</h4><p>");
        html.push_str(&escape(&finding.summary));
        html.push_str("</p>");
        if !finding.evidence.is_empty() {
            html.push_str(r#"<ul class="metric-list">"#);
            for evidence in &finding.evidence {
                html.push_str("<li><strong>");
                html.push_str(metric_label(&evidence.metric));
                html.push_str(":</strong> ");
                html.push_str(&evidence.value.to_string());
                html.push_str(r#" <span class="muted">(comparison reference "#);
                html.push_str(&evidence.reference.to_string());
                html.push_str(" across ");
                html.push_str(&evidence.population.to_string());
                html.push_str(" comparable modules");
                if let Some(baseline) = evidence.baseline {
                    html.push_str(", baseline ");
                    html.push_str(&baseline.to_string());
                }
                if let Some(material) = evidence.material_delta {
                    html.push_str(", material growth threshold ");
                    html.push_str(&material.to_string());
                }
                html.push_str(")</span></li>");
            }
            html.push_str("</ul>");
        }

        if let Some(module) = modules
            .iter()
            .find(|module| module_subject(module) == finding.subject)
        {
            let anchor = anchor_id("module", &finding.subject);
            html.push_str(r#"<p class="location"><strong>Affected area:</strong> <code>"#);
            html.push_str(&escape(&module.path));
            html.push_str(r##"</code> · <a href="#"##);
            html.push_str(&anchor);
            html.push_str(r#"">View in repository explorer</a></p>"#);
        }

        let contexts = contexts_for_finding(finding, source_contexts);
        if !contexts.is_empty() {
            html.push_str(
                r#"<details class="source-evidence"><summary>Relevant source evidence ("#,
            );
            html.push_str(&contexts.len().to_string());
            html.push_str(")</summary>");
            for context in contexts {
                html.push_str(r#"<div class="source-context"><p><strong>"#);
                html.push_str(metric_label(&context.metric));
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

        html.push_str("<h4>What to investigate next</h4><p>");
        html.push_str(&escape(&finding.direction));
        html.push_str("</p>");

        if let Some(reason) = &finding.acceptance_reason {
            html.push_str("<p><strong>Acceptance:</strong> ");
            html.push_str(&escape(reason));
            html.push_str("</p>");
        }

        html.push_str(
            "<details><summary>Technical details</summary><p><strong>Rule:</strong> <code>",
        );
        html.push_str(&escape(&finding.rule));
        html.push_str("</code><br><strong>Fingerprint:</strong> <code>");
        html.push_str(&escape(&finding.fingerprint));
        html.push_str("</code><br><strong>Configuration:</strong> <code>");
        html.push_str(&escape(&finding.configuration));
        html.push_str("</code></p></details></article>");
    }
    html
}

fn metric_label(metric: &str) -> &'static str {
    match metric {
        "decision_sites" => "Decision points",
        "local_dependency_modules" => "Repository dependencies",
        "clone_call_syntax_sites" => "Clone call sites",
        "reverse_repository_dependents" => "Modules depending on this area",
        "public_items" => "Public items",
        _ => "Evidence value",
    }
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
        assert!(rendered.contains("explicit-import dependency cycle"));
        assert!(rendered.contains("Sample truncated"));
        assert!(rendered.contains("unattached context only"));
        assert!(rendered.contains("&lt;accepted&gt;"));
        assert!(rendered.contains("Often changed with"));

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
        assert!(rendered.contains("Repository explorer"));
        assert!(rendered.contains("a/src/lib.rs"));
        assert!(rendered.contains("b/src/lib.rs"));
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

        assert!(rendered.contains("Possible refactoring opportunity"));
        assert!(rendered.contains("This module may be harder to change safely"));
        assert!(rendered.contains("Repeated copying may be worth measuring"));
        assert!(rendered.contains("Changes here may affect many parts of the repository"));
        assert!(rendered.contains("<h2>What needs attention</h2>"));
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

        assert!(rendered.contains("<h2>What needs attention</h2>"));
        assert!(rendered.contains("1 area worth reviewing"));
        assert!(rendered.contains("1 investigate"));
        assert!(rendered.contains("1 observe"));
        assert!(rendered.contains("Possible refactoring opportunity"));
        assert!(rendered.contains("Repeated copying may be worth measuring"));
        assert!(rendered.contains("Priority 2 — investigate"));
        assert!(rendered.contains("strong evidence"));
        assert!(rendered.contains("worsened"));
        assert!(rendered.contains("src/engine.rs"));
        assert!(rendered.contains(">View in repository explorer</a>"));
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
