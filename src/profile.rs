use std::process::Command;

use crate::{cfg::HostCfg, model::AnalysisProfile};

#[derive(Debug, Clone)]
pub struct ProfileContext {
    pub public: AnalysisProfile,
    pub cfg: HostCfg,
}

impl ProfileContext {
    pub fn resolve(target: Option<&str>, features: &[String]) -> Result<Self, String> {
        let resolved_target = match target {
            Some(target) if !target.trim().is_empty() => target.trim().to_owned(),
            _ => rustc_host()?,
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
            format!("target={target_label};features=default")
        } else {
            format!(
                "target={target_label};features=default+{}",
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
    let output = Command::new("rustc")
        .arg("-vV")
        .output()
        .map_err(|error| format!("could not execute rustc -vV: {error}"))?;
    parse_rustc_host_output(&output)
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
mod tests {
    use std::process::Command;

    use super::{parse_rustc_host_output, ProfileContext};

    #[test]
    fn normalizes_explicit_profile_inputs() {
        let features = vec![" z ".into(), "a".into(), "a".into(), " ".into()];
        let profile =
            ProfileContext::resolve(Some(" x86_64-unknown-linux-gnu "), &features).unwrap();

        assert_eq!(profile.public.target, "x86_64-unknown-linux-gnu");
        assert_eq!(profile.public.features, ["a", "z"]);
        assert!(profile.public.id.contains("default+a,z"));
    }

    #[test]
    fn rustc_host_parser_reports_command_failure_and_missing_host() {
        let failed = Command::new("rustc")
            .arg("--definitely-invalid-ferric-lens-option")
            .output()
            .unwrap();
        assert!(parse_rustc_host_output(&failed).is_err());

        let version_only = Command::new("rustc").arg("--version").output().unwrap();
        assert!(parse_rustc_host_output(&version_only)
            .unwrap_err()
            .contains("did not report a host target"));
    }
}
