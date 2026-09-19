use std::{
    collections::{BTreeMap, BTreeSet},
    process::Command,
};

use syn::{punctuated::Punctuated, Expr, ExprLit, Lit, Meta, Token};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Truth {
    True,
    False,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct HostCfg {
    flags: BTreeSet<String>,
    values: BTreeMap<String, BTreeSet<String>>,
    digest: String,
    reliable: bool,
    explicit_features: BTreeSet<String>,
}

impl HostCfg {
    pub fn detect() -> Result<Self, String> {
        Self::detect_for_target(None, &[])
    }

    pub fn detect_for_target(target: Option<&str>, features: &[String]) -> Result<Self, String> {
        let mut command = Command::new("rustc");
        command.args(["--print", "cfg"]);
        if let Some(target) = target {
            command.args(["--target", target]);
        }
        let output = command
            .output()
            .map_err(|error| format!("could not execute rustc --print cfg: {error}"))?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
        }

        let text = std::str::from_utf8(&output.stdout)
            .map_err(|error| format!("rustc cfg output is not UTF-8: {error}"))?;
        let mut cfg = Self::from_lines(text.lines())?;
        cfg.explicit_features = features
            .iter()
            .map(|feature| feature.trim().to_owned())
            .filter(|feature| !feature.is_empty())
            .collect();
        cfg.rehash();
        Ok(cfg)
    }

    pub fn unavailable() -> Self {
        Self {
            flags: BTreeSet::new(),
            values: BTreeMap::new(),
            digest: "cfg-unavailable".into(),
            reliable: false,
            explicit_features: BTreeSet::new(),
        }
    }

    pub fn digest(&self) -> &str {
        &self.digest
    }

    pub fn canonical_lines(&self) -> Vec<String> {
        let mut lines = self.flags.iter().cloned().collect::<Vec<_>>();
        for (key, values) in &self.values {
            for value in values {
                lines.push(format!("{key}=\"{value}\""));
            }
        }
        lines.sort();
        lines
    }

    pub fn evaluate(&self, meta: &Meta) -> Truth {
        if !self.reliable {
            if matches!(meta, Meta::Path(path) if path.is_ident("test")) {
                return Truth::False;
            }
            return Truth::Unknown;
        }

        match meta {
            Meta::Path(path) => {
                let Some(ident) = path.get_ident() else {
                    return Truth::Unknown;
                };
                let key = ident.to_string();
                if key == "test" {
                    Truth::False
                } else if self.flags.contains(&key) {
                    Truth::True
                } else if is_known_bare_cfg(&key) {
                    Truth::False
                } else {
                    Truth::Unknown
                }
            }
            Meta::NameValue(name_value) => {
                let Some(ident) = name_value.path.get_ident() else {
                    return Truth::Unknown;
                };
                let key = ident.to_string();
                if key == "feature" {
                    let Expr::Lit(ExprLit {
                        lit: Lit::Str(value),
                        ..
                    }) = &name_value.value
                    else {
                        return Truth::Unknown;
                    };
                    return if self.explicit_features.contains(&value.value()) {
                        Truth::True
                    } else {
                        Truth::Unknown
                    };
                }
                let Expr::Lit(ExprLit {
                    lit: Lit::Str(value),
                    ..
                }) = &name_value.value
                else {
                    return Truth::Unknown;
                };

                match self.values.get(&key) {
                    Some(values) if values.contains(&value.value()) => Truth::True,
                    Some(_) => Truth::False,
                    None if is_known_value_cfg(&key) => Truth::False,
                    None => Truth::Unknown,
                }
            }
            Meta::List(list) => {
                let Some(ident) = list.path.get_ident() else {
                    return Truth::Unknown;
                };
                let nested =
                    match list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated) {
                        Ok(nested) => nested,
                        Err(_) => return Truth::Unknown,
                    };

                match ident.to_string().as_str() {
                    "all" => all(nested.iter().map(|meta| self.evaluate(meta))),
                    "any" => any(nested.iter().map(|meta| self.evaluate(meta))),
                    "not" if nested.len() == 1 => negate(self.evaluate(&nested[0])),
                    _ => Truth::Unknown,
                }
            }
        }
    }

    fn from_lines<'a>(lines: impl IntoIterator<Item = &'a str>) -> Result<Self, String> {
        let mut flags = BTreeSet::new();
        let mut values = BTreeMap::<String, BTreeSet<String>>::new();

        for raw in lines {
            let line = raw.trim();
            if line.is_empty() {
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                let value = value.trim();
                let Some(value) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) else {
                    return Err(format!("unsupported rustc cfg value: {line}"));
                };
                values
                    .entry(key.trim().to_owned())
                    .or_default()
                    .insert(value.to_owned());
            } else {
                flags.insert(line.to_owned());
            }
        }

        let mut cfg = Self {
            flags,
            values,
            digest: String::new(),
            reliable: true,
            explicit_features: BTreeSet::new(),
        };
        cfg.rehash();
        Ok(cfg)
    }

    fn rehash(&mut self) {
        let mut hasher = blake3::Hasher::new();
        for flag in &self.flags {
            hasher.update(flag.as_bytes());
            hasher.update(&[0]);
        }
        for (key, set) in &self.values {
            hasher.update(key.as_bytes());
            hasher.update(&[1]);
            for value in set {
                hasher.update(value.as_bytes());
                hasher.update(&[0]);
            }
        }
        for feature in &self.explicit_features {
            hasher.update(b"feature");
            hasher.update(&[2]);
            hasher.update(feature.as_bytes());
            hasher.update(&[0]);
        }
        self.digest = hasher.finalize().to_hex().to_string();
    }

    #[cfg(test)]
    pub fn test(lines: &[&str]) -> Self {
        Self::from_lines(lines.iter().copied()).unwrap()
    }

    #[cfg(test)]
    pub fn test_with_features(lines: &[&str], features: &[&str]) -> Self {
        let mut cfg = Self::from_lines(lines.iter().copied()).unwrap();
        cfg.explicit_features = features
            .iter()
            .map(|feature| (*feature).to_owned())
            .collect();
        cfg.rehash();
        cfg
    }
}

fn all(values: impl IntoIterator<Item = Truth>) -> Truth {
    let mut saw_unknown = false;
    for value in values {
        match value {
            Truth::False => return Truth::False,
            Truth::Unknown => saw_unknown = true,
            Truth::True => {}
        }
    }
    if saw_unknown {
        Truth::Unknown
    } else {
        Truth::True
    }
}

fn any(values: impl IntoIterator<Item = Truth>) -> Truth {
    let mut saw_unknown = false;
    for value in values {
        match value {
            Truth::True => return Truth::True,
            Truth::Unknown => saw_unknown = true,
            Truth::False => {}
        }
    }
    if saw_unknown {
        Truth::Unknown
    } else {
        Truth::False
    }
}

fn negate(value: Truth) -> Truth {
    match value {
        Truth::True => Truth::False,
        Truth::False => Truth::True,
        Truth::Unknown => Truth::Unknown,
    }
}

fn is_known_bare_cfg(key: &str) -> bool {
    matches!(
        key,
        "unix" | "windows" | "debug_assertions" | "proc_macro" | "target_thread_local" | "doctest"
    )
}

fn is_known_value_cfg(key: &str) -> bool {
    matches!(
        key,
        "target_arch"
            | "target_endian"
            | "target_env"
            | "target_family"
            | "target_feature"
            | "target_has_atomic"
            | "target_os"
            | "target_pointer_width"
            | "target_vendor"
            | "panic"
    )
}

#[cfg(test)]
mod tests {
    use syn::parse_quote;

    use super::{HostCfg, Truth};

    fn linux() -> HostCfg {
        HostCfg::test(&[
            "unix",
            "target_arch=\"x86_64\"",
            "target_family=\"unix\"",
            "target_os=\"linux\"",
            "target_pointer_width=\"64\"",
        ])
    }

    #[test]
    fn evaluates_standard_host_predicates() {
        let cfg = linux();
        assert_eq!(cfg.evaluate(&parse_quote!(unix)), Truth::True);
        assert_eq!(
            cfg.evaluate(&parse_quote!(target_os = "linux")),
            Truth::True
        );
        assert_eq!(
            cfg.evaluate(&parse_quote!(target_os = "macos")),
            Truth::False
        );
        assert_eq!(cfg.evaluate(&parse_quote!(windows)), Truth::False);
        assert_eq!(cfg.evaluate(&parse_quote!(test)), Truth::False);
    }

    #[test]
    fn evaluates_boolean_cfg_combinators() {
        let cfg = linux();
        assert_eq!(
            cfg.evaluate(&parse_quote!(all(unix, not(windows)))),
            Truth::True
        );
        assert_eq!(
            cfg.evaluate(&parse_quote!(any(target_os = "macos", windows))),
            Truth::False
        );
    }

    #[test]
    fn preserves_unknown_custom_and_unselected_feature_cfg() {
        let cfg = linux();
        assert_eq!(
            cfg.evaluate(&parse_quote!(feature = "fast")),
            Truth::Unknown
        );
        assert_eq!(cfg.evaluate(&parse_quote!(my_custom_cfg)), Truth::Unknown);
        assert_eq!(
            cfg.evaluate(&parse_quote!(any(my_custom_cfg, windows))),
            Truth::Unknown
        );
    }

    #[test]
    fn selected_explicit_feature_is_true() {
        let cfg = HostCfg::test_with_features(&["unix", "target_os=\"linux\""], &["fast"]);
        assert_eq!(cfg.evaluate(&parse_quote!(feature = "fast")), Truth::True);
        assert_eq!(
            cfg.evaluate(&parse_quote!(feature = "other")),
            Truth::Unknown
        );
    }

    #[test]
    fn detects_host_cfg_and_rejects_an_invalid_target() {
        let detected = HostCfg::detect().unwrap();
        assert!(!detected.digest().is_empty());
        assert!(!detected.canonical_lines().is_empty());

        let error =
            HostCfg::detect_for_target(Some("ferric-lens-invalid-target"), &[]).unwrap_err();
        assert!(!error.is_empty());
    }

    #[test]
    fn unavailable_cfg_never_invents_non_test_truth() {
        let cfg = HostCfg::unavailable();
        assert_eq!(cfg.digest(), "cfg-unavailable");
        assert!(cfg.canonical_lines().is_empty());
        assert_eq!(cfg.evaluate(&parse_quote!(test)), Truth::False);
        assert_eq!(cfg.evaluate(&parse_quote!(unix)), Truth::Unknown);
    }

    #[test]
    fn malformed_or_semantically_unknown_meta_is_unknown() {
        let cfg = linux();

        assert_eq!(cfg.evaluate(&parse_quote!(foo::bar)), Truth::Unknown);
        assert_eq!(
            cfg.evaluate(&parse_quote!(foo::bar = "value")),
            Truth::Unknown
        );
        assert_eq!(cfg.evaluate(&parse_quote!(feature = SOME)), Truth::Unknown);
        assert_eq!(
            cfg.evaluate(&parse_quote!(target_os = SOME)),
            Truth::Unknown
        );
        assert_eq!(cfg.evaluate(&parse_quote!(foo::bar(unix))), Truth::Unknown);

        let malformed: syn::Meta = syn::parse_str("all(@)").unwrap();
        assert_eq!(cfg.evaluate(&malformed), Truth::Unknown);
        assert_eq!(cfg.evaluate(&parse_quote!(xor(unix))), Truth::Unknown);
        assert_eq!(cfg.evaluate(&parse_quote!(not(unix, windows))), Truth::Unknown);
    }

    #[test]
    fn boolean_cfg_operators_cover_true_false_and_unknown_paths() {
        let cfg = linux();

        assert_eq!(
            cfg.evaluate(&parse_quote!(all(unix, windows))),
            Truth::False
        );
        assert_eq!(
            cfg.evaluate(&parse_quote!(all(unix, my_custom_cfg))),
            Truth::Unknown
        );
        assert_eq!(
            cfg.evaluate(&parse_quote!(any(windows, unix))),
            Truth::True
        );
        assert_eq!(
            cfg.evaluate(&parse_quote!(not(my_custom_cfg))),
            Truth::Unknown
        );
    }

    #[test]
    fn parses_cfg_lines_canonically_and_rejects_unquoted_values() {
        let cfg = HostCfg::from_lines([
            "",
            "unix",
            "target_os=\"linux\"",
            "target_feature=\"sse2\"",
            "target_feature=\"avx\"",
        ])
        .unwrap();

        assert_eq!(
            cfg.canonical_lines(),
            [
                "target_feature=\"avx\"",
                "target_feature=\"sse2\"",
                "target_os=\"linux\"",
                "unix",
            ]
        );
        assert!(HostCfg::from_lines(["target_os=linux"]).is_err());
    }

    #[test]
    fn recognizes_every_builtin_cfg_family() {
        for key in [
            "unix",
            "windows",
            "debug_assertions",
            "proc_macro",
            "target_thread_local",
            "doctest",
        ] {
            assert!(super::is_known_bare_cfg(key));
        }
        assert!(!super::is_known_bare_cfg("custom"));

        for key in [
            "target_arch",
            "target_endian",
            "target_env",
            "target_family",
            "target_feature",
            "target_has_atomic",
            "target_os",
            "target_pointer_width",
            "target_vendor",
            "panic",
        ] {
            assert!(super::is_known_value_cfg(key));
        }
        assert!(!super::is_known_value_cfg("custom"));
    }

}
