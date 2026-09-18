use std::{fs, path::Path};

use crate::model::{AnalysisResult, GateVerdict};

pub fn json(result: &AnalysisResult) -> Result<String, String> {
    serde_json::to_string_pretty(result)
        .map_err(|error| format!("cannot serialize JSON: {error}"))
}

pub fn html(result: &AnalysisResult) -> String {
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
        capabilities.push_str(&escape(
            &format!("{:?}", capability.status).to_lowercase(),
        ));
        if let Some(detail) = &capability.detail {
            capabilities.push_str(" — ");
            capabilities.push_str(&escape(detail));
        }
        capabilities.push_str("</li>");
    }

    let mut findings = String::new();
    if result.findings.is_empty() {
        findings.push_str(
            "<p>No advisory coupled outliers were found in eligible crate populations.</p>",
        );
    } else {
        for finding in &result.findings {
            findings.push_str("<article><h3>");
            findings.push_str(&escape(&finding.subject));
            findings.push_str("</h3><p>");
            findings.push_str(&escape(&finding.summary));
            findings.push_str("</p><ul>");
            for evidence in &finding.evidence {
                findings.push_str("<li>");
                findings.push_str(&escape(&format!(
                    "{} = {} (p90 reference {}, population {})",
                    evidence.metric, evidence.value, evidence.reference, evidence.population
                )));
                findings.push_str("</li>");
            }
            findings.push_str("</ul><p><strong>Direction:</strong> ");
            findings.push_str(&escape(&finding.direction));
            findings.push_str("</p></article>");
        }
    }

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
            modules.push_str("</strong></summary><div class="crate">");
        }
        modules.push_str("<div class="module"><code>");
        modules.push_str(&escape(if module.module_path.is_empty() {
            "crate root"
        } else {
            &module.module_path
        }));
        modules.push_str("</code><span>");
        modules.push_str(&escape(&format!(
            "{} decisions · {} local deps · {} public items · {} lines",
            module.decision_sites,
            module.local_dependency_modules.len(),
            module.public_items,
            module.lines
        )));
        modules.push_str("</span></div>");
    }
    if !current_crate.is_empty() {
        modules.push_str("</div></details>");
    }

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
summary {{ cursor: pointer; }}
.module {{ display: flex; justify-content: space-between; gap: 1rem; padding: .35rem 0; border-bottom: 1px solid color-mix(in srgb, currentColor 10%, transparent); }}
.module span {{ text-align: right; opacity: .8; }}
code {{ overflow-wrap: anywhere; }}
</style>
</head>
<body>
<header><h1>Ferric Lens</h1><p class="verdict">{verdict}</p><p>{reason}</p></header>
<section class="grid">
<div class="card"><h2>Snapshot</h2><p><strong>Digest</strong><br><code>{digest}</code></p><p>{source_files} source files</p></div>
<div class="card"><h2>Coverage</h2><ul>{capabilities}</ul></div>
</section>
<section><h2>Advisory findings</h2>{findings}</section>
<section><h2>Codebase map</h2>{modules}</section>
</body></html>"#,
        reason = escape(&result.verdict_reason),
        digest = escape(&result.snapshot.content_digest),
        source_files = result.snapshot.source_files,
    )
}

pub fn write(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    fs::write(path, contents)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))
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
    use super::escape;

    #[test]
    fn escapes_html_metacharacters() {
        assert_eq!(
            escape("<a x='&'>\""),
            "&lt;a x=&#39;&amp;&#39;&gt;&quot;"
        );
    }
}
