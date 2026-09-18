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
        #[arg(long, default_value = "ferric-lens.json")]
        json: PathBuf,
        #[arg(long, default_value = "ferric-lens-report.html")]
        html: PathBuf,
    },
    /// Run the CI-oriented analysis path.
    ///
    /// The foundation implementation returns exit code 2 because baseline
    /// comparison is not implemented yet; current-snapshot analysis is still
    /// emitted when requested.
    Check {
        #[arg(default_value = ".")]
        path: PathBuf,
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
        Command::Analyze { path, json, html } => {
            let result = ferric_lens::analyze(&path)?;
            let json_text = report::json(&result)?;
            report::write(&json, &json_text)?;
            report::write(&html, &report::html(&result))?;
            print_summary(&result);
            Ok(exit_code(&result.verdict))
        }
        Command::Check { path, json } => {
            let result = ferric_lens::analyze(&path)?;
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
    println!(
        "{} source files, {} advisory findings",
        result.snapshot.source_files,
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
