use std::{
    collections::{BTreeMap, BTreeSet},
    process::Command,
};

use crate::model::AnalysisProfile;

#[derive(Debug, Clone)]
pub struct ProfileContext {
    pub public: AnalysisProfile,
    flags: BTreeSet<String>,
    values: BTreeMap<String, BTreeSet<String>>,
    explicit_features: BTreeSet<String>,
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

        let target_cfg = rustc_cfg(&resolved_target)?;
        let (flags, values) = parse_cfg_lines(&target_cfg);
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
                features: normalized_features.clone(),
                target_cfg,
            },
            flags,
            values,
            explicit_features: normalized_features.into_iter().collect(),
        })
    }

    pub fn flag(&self, name: &str) -> bool {
        self.flags.contains(name)
    }

    pub fn value(&self, key: &str, value: &str) -> bool {
        self.values
            .get(key)
            .is_some_and(|values| values.contains(value))
    }

    pub fn explicit_feature(&self, feature: &str) -> bool {
        self.explicit_features.contains(feature)
    }
}

fn rustc_host() -> Result<String, String> {
    let output = Command::new("rustc")
        .arg("-vV")
        .output()
        .map_err(|error| format!("could not execute rustc -vV: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .find_map(|line| line.strip_prefix("host: ").map(str::to_owned))
        .ok_or_else(|| "rustc -vV did not report a host target".into())
}

fn rustc_cfg(target: &str) -> Result<Vec<String>, String> {
    let output = Command::new("rustc")
        .args(["--print", "cfg", "--target", target])
        .output()
        .map_err(|error| format!("could not execute rustc --print cfg: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }

    let mut lines = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    lines.sort();
    lines.dedup();
    Ok(lines)
}

fn parse_cfg_lines(lines: &[String]) -> (BTreeSet<String>, BTreeMap<String, BTreeSet<String>>) {
    let mut flags = BTreeSet::new();
    let mut values = BTreeMap::<String, BTreeSet<String>>::new();

    for line in lines {
        if let Some((key, raw_value)) = line.split_once('=') {
            let value = raw_value.trim_matches('"');
            values
                .entry(key.to_owned())
                .or_default()
                .insert(value.to_owned());
        } else {
            flags.insert(line.clone());
        }
    }

    (flags, values)
}

#[cfg(test)]
mod tests {
    use super::parse_cfg_lines;

    #[test]
    fn parses_rustc_cfg_facts_deterministically() {
        let lines = vec![
            "unix".to_owned(),
            "target_os=\"linux\"".to_owned(),
            "target_arch=\"x86_64\"".to_owned(),
        ];
        let (flags, values) = parse_cfg_lines(&lines);
        assert!(flags.contains("unix"));
        assert!(values["target_os"].contains("linux"));
        assert!(values["target_arch"].contains("x86_64"));
    }
}
