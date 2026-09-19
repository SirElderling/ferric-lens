use std::process::Command;

use crate::{cfg::HostCfg, model::AnalysisProfile};

#[derive(Debug, Clone)]
pub struct ProfileContext {
    pub public: AnalysisProfile,
    pub cfg: HostCfg,
}

impl ProfileContext {
    pub fn resolve(target: Option<&str>, features: &[String]) -> Result<Self, String> {
        Self::resolve_with_host(target, features, rustc_host)
    }

    fn resolve_with_host<F>(
        target: Option<&str>,
        features: &[String],
        host: F,
    ) -> Result<Self, String>
    where
        F: FnOnce() -> Result<String, String>,
    {
        let resolved_target = match target {
            Some(target) if !target.trim().is_empty() => target.trim().to_owned(),
            _ => host()?,
        };

        let mut normalized_features = features
            .iter()
            .map(|feature| feature.trim().to_owned())
            .filter(|feature| !feature.is_empty())
            .collect::<Vec<_>>();
        normalized_features.sort();
        normalized_features.dedup();

        let cfg = HostCfg::detect_for_target(Some(&resolved_target), &normalized_features)?;
        let target_label = target
            .filter(|target| !target.trim().is_empty())
            .map(str::trim)
            .unwrap_or("host")
            .to_owned();
        let id = if normalized_features.is_empty() {
            format!("target={resolved_target};features=default")
        } else {
            format!(
                "target={resolved_target};features=default+{}",
                normalized_features.join(",")
            )
        };

        Ok(Self {
            public: AnalysisProfile {
                id,
                target: target_label,
                resolved_target,
                features: normalized_features,
                target_cfg: cfg.canonical_lines(),
            },
            cfg,
        })
    }
}

fn rustc_host() -> Result<String, String> {
    let mut command = Command::new("rustc");
    command.arg("-vV");
    run_rustc_host_and_parse(&mut command)
}

fn run_rustc_host_and_parse(command: &mut Command) -> Result<String, String> {
    let output = run_rustc_host(command)?;
    parse_rustc_host_output(&output)
}

fn run_rustc_host(command: &mut Command) -> Result<std::process::Output, String> {
    match command.output() {
        Ok(output) => Ok(output),
        Err(error) => Err(format!("could not execute rustc -vV: {error}")),
    }
}

fn parse_rustc_host_output(output: &std::process::Output) -> Result<String, String> {
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(if stderr.is_empty() {
            format!("rustc -vV failed with status {}", output.status)
        } else {
            stderr
        });
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .find_map(|line| line.strip_prefix("host: ").map(str::to_owned))
        .ok_or_else(|| "rustc -vV did not report a host target".into())
}

#[cfg(test)]
#[path = "profile_tests.rs"]
mod tests;
