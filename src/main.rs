use std::{path::PathBuf, process::ExitCode};

use clap::{Parser, Subcommand};
use ferric_lens::{model::GateVerdict, report};

#[derive(Debug, Parser)]
#[command(name = "ferric-lens", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Analyze the current repository and write JSON plus self-contained HTML.
    Analyze {
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Explicit target ref to compare against. The merge base is analyzed.
        #[arg(long)]
        base: Option<String>,
        /// Rust target triple. Defaults to the host target.
        #[arg(long)]
        target: Option<String>,
        /// Additional Cargo feature to enable; may be repeated.
        #[arg(long = "feature")]
        features: Vec<String>,
        #[arg(long)]
        json: Option<PathBuf>,
        #[arg(long)]
        html: Option<PathBuf>,
        /// Optional normalized local evidence envelope for report enrichment.
        #[arg(long)]
        evidence: Option<PathBuf>,
        /// Print compact deterministic JSON for AI/agent consumption instead of the human summary.
        #[arg(long)]
        ai: bool,
    },
    /// Record an explicit acceptance for one current finding.
    Accept {
        /// Exact finding fingerprint from the current analysis.
        fingerprint: String,
        /// Plain-English reason for accepting this exact finding.
        #[arg(long)]
        reason: String,
        /// Repository to analyze.
        #[arg(long, default_value = ".")]
        path: PathBuf,
        /// Explicit target ref to compare against. The merge base is analyzed.
        #[arg(long)]
        base: Option<String>,
        /// Rust target triple. Defaults to the host target.
        #[arg(long)]
        target: Option<String>,
        /// Additional Cargo feature to enable; may be repeated.
        #[arg(long = "feature")]
        features: Vec<String>,
    },
    /// Run the CI-oriented analysis path.
    Check {
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Explicit target ref to compare against. The merge base is analyzed.
        #[arg(long)]
        base: Option<String>,
        /// Rust target triple. Defaults to the host target.
        #[arg(long)]
        target: Option<String>,
        /// Additional Cargo feature to enable; may be repeated.
        #[arg(long = "feature")]
        features: Vec<String>,
        #[arg(long)]
        json: Option<PathBuf>,
        /// Print compact deterministic JSON for AI/agent consumption instead of the human summary.
        #[arg(long)]
        ai: bool,
    },
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("ferric-lens: {error}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode, String> {
    match cli.command {
        Command::Analyze {
            path,
            base,
            target,
            features,
            json,
            html,
            evidence,
            ai,
        } => {
            let result = ferric_lens::analyze_with_profile(
                &path,
                base.as_deref(),
                target.as_deref(),
                &features,
                evidence.as_deref(),
            )?;
            let (json, html) = analyze_artifact_paths(ai, json, html);
            if let Some(path) = json {
                report::write(&path, &report::json(&result))?;
            }
            if let Some(path) = html {
                report::write(&path, &report::html(&result))?;
            }
            print!("{}", output_text(&result, ai));
            Ok(exit_code(&result.verdict))
        }
        Command::Accept {
            fingerprint,
            reason,
            path,
            base,
            target,
            features,
        } => {
            ferric_lens::accept_finding_with_profile(
                &path,
                base.as_deref(),
                target.as_deref(),
                &features,
                &fingerprint,
                &reason,
            )?;
            println!("accepted finding {fingerprint}");
            Ok(ExitCode::SUCCESS)
        }
        Command::Check {
            path,
            base,
            target,
            features,
            json,
            ai,
        } => {
            let result = ferric_lens::check_with_profile(
                &path,
                base.as_deref(),
                target.as_deref(),
                &features,
            )?;
            if let Some(path) = json {
                report::write(&path, &report::json(&result))?;
            }
            print!("{}", output_text(&result, ai));
            Ok(exit_code(&result.verdict))
        }
    }
}

fn analyze_artifact_paths(
    ai: bool,
    json: Option<PathBuf>,
    html: Option<PathBuf>,
) -> (Option<PathBuf>, Option<PathBuf>) {
    if ai {
        (json, html)
    } else {
        (
            Some(json.unwrap_or_else(|| PathBuf::from("ferric-lens.json"))),
            Some(html.unwrap_or_else(|| PathBuf::from("ferric-lens-report.html"))),
        )
    }
}

fn output_text(result: &ferric_lens::model::AnalysisResult, ai: bool) -> String {
    if ai {
        format!("{}\n", report::ai_json(result))
    } else {
        report::cli_summary(result)
    }
}

fn exit_code(verdict: &GateVerdict) -> ExitCode {
    match verdict {
        GateVerdict::Pass => ExitCode::SUCCESS,
        GateVerdict::Regression => ExitCode::from(1),
        GateVerdict::Inconclusive => ExitCode::from(2),
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    use ferric_lens::model::{
        AnalysisProfile, AnalysisResult, ArchitectureSummary, GateVerdict, Snapshot,
    };

    use super::{analyze_artifact_paths, output_text, Cli, Command};

    fn result() -> AnalysisResult {
        AnalysisResult {
            schema_version: 1,
            tool_version: "test".into(),
            snapshot: Snapshot {
                content_digest: "source".into(),
                git_head: None,
                dirty: Some(false),
                source_files: 1,
            },
            profile: AnalysisProfile {
                id: "host".into(),
                target: "host".into(),
                resolved_target: "test-target".into(),
                features: Vec::new(),
                target_cfg: Vec::new(),
            },
            baseline: None,
            verdict: GateVerdict::Pass,
            verdict_reason: "complete".into(),
            applicable_gate_subjects: 0,
            architecture: ArchitectureSummary {
                modules: 1,
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
    fn analyze_ai_is_stdout_only_by_default_but_honors_explicit_artifacts() {
        let (json, html) = analyze_artifact_paths(true, None, None);
        assert!(json.is_none());
        assert!(html.is_none());

        let explicit_json = PathBuf::from("out.json");
        let explicit_html = PathBuf::from("out.html");
        let (json, html) = analyze_artifact_paths(
            true,
            Some(explicit_json.clone()),
            Some(explicit_html.clone()),
        );
        assert_eq!(json, Some(explicit_json));
        assert_eq!(html, Some(explicit_html));

        let (json, html) = analyze_artifact_paths(false, None, None);
        assert_eq!(json, Some(PathBuf::from("ferric-lens.json")));
        assert_eq!(html, Some(PathBuf::from("ferric-lens-report.html")));
    }

    #[test]
    fn analyze_and_check_accept_ai_output_mode() {
        let analyze = Cli::try_parse_from(["ferric-lens", "analyze", ".", "--ai"]).unwrap();
        assert!(matches!(analyze.command, Command::Analyze { ai: true, .. }));

        let check = Cli::try_parse_from(["ferric-lens", "check", ".", "--ai"]).unwrap();
        assert!(matches!(check.command, Command::Check { ai: true, .. }));
    }

    #[test]
    fn output_mode_switches_between_human_and_ai_views() {
        let result = result();
        let human = output_text(&result, false);
        let ai = output_text(&result, true);

        assert!(human.contains("Ferric Lens: Pass"));
        assert!(human.contains("areas worth reviewing"));
        assert!(ai.contains("\"format\": \"ferric_lens_ai\""));
        assert!(!ai.contains("Ferric Lens: PASS"));
    }
}
