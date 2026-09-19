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
        let output = run_rustc_cfg(&mut command)?;
        let mut cfg = parse_rustc_cfg(&output.stdout)?;
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
}


fn run_rustc_cfg(command: &mut Command) -> Result<std::process::Output, String> {
    let output = match command.output() {
        Ok(output) => output,
        Err(error) => return Err(format!("could not execute rustc --print cfg: {error}")),
    };
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    Ok(output)
}

fn parse_rustc_cfg(bytes: &[u8]) -> Result<HostCfg, String> {
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) => return Err(format!("rustc cfg output is not UTF-8: {error}")),
    };
    HostCfg::from_lines(text.lines())
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
#[path = "cfg_tests.rs"]
mod tests;
