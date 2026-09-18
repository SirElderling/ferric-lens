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
        #[arg(long, default_value = "ferric-lens.json")]
        json: PathBuf,
        #[arg(long, default_value = "ferric-lens-report.html")]
        html: PathBuf,
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
    },
    /// Run the CI-oriented analysis path.
    Check {
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Explicit target ref to compare against. The merge base is analyzed.
        #[arg(long)]
        base: Option<String>,
        #[arg(long)]
        json: Option<PathBuf>,
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
            json,
            html,
        } => {
            let result = ferric_lens::analyze_with_base(&path, base.as_deref())?;
            let json_text = report::json(&result)?;
            report::write(&json, &json_text)?;
            report::write(&html, &report::html(&result))?;
            print_summary(&result);
            Ok(exit_code(&result.verdict))
        }
        Command::Accept {
            fingerprint,
            reason,
            path,
            base,
        } => {
            ferric_lens::accept_finding_with_base(
                &path,
                base.as_deref(),
                &fingerprint,
                &reason,
            )?;
            println!("accepted finding {fingerprint}");
            Ok(ExitCode::SUCCESS)
        }
        Command::Check { path, base, json } => {
            let result = ferric_lens::analyze_with_base(&path, base.as_deref())?;
            if let Some(path) = json {
                report::write(&path, &report::json(&result)?)?;
            }
            print_summary(&result);
            Ok(exit_code(&result.verdict))
        }
    }
}

fn print_summary(result: &ferric_lens::model::AnalysisResult) {
    println!("Ferric Lens: {:?}", result.verdict);
    println!("{}", result.verdict_reason);
    if let Some(baseline) = &result.baseline {
        println!(
            "baseline {} -> merge base {}",
            baseline.target_ref, baseline.merge_base
        );
    }
    let gate_findings = result
        .findings
        .iter()
        .filter(|finding| finding.gate && !finding.accepted)
        .count();
    let accepted = result
        .findings
        .iter()
        .filter(|finding| finding.accepted)
        .count();
    println!(
        "{} source files, {} applicable gate subjects, {} unaccepted gate findings, {} accepted findings, {} total findings",
        result.snapshot.source_files,
        result.applicable_gate_subjects,
        gate_findings,
        accepted,
        result.findings.len()
    );
}

fn exit_code(verdict: &GateVerdict) -> ExitCode {
    match verdict {
        GateVerdict::Pass => ExitCode::SUCCESS,
        GateVerdict::Regression => ExitCode::from(1),
        GateVerdict::Inconclusive => ExitCode::from(2),
    }
}
