// ferric-lens: ignore-correctness-risks
use syn::{spanned::Spanned, Item};

use crate::{
    input::SourceFile,
    model::{DeltaStatus, Evidence, EvidenceClass, Finding, Priority, SourceContext},
};

#[derive(Debug, Default)]
pub struct CorrectnessScan {
    pub findings: Vec<Finding>,
    pub source_contexts: Vec<SourceContext>,
}

#[derive(Debug, Clone)]
struct Match {
    path: String,
    line: usize,
    excerpt: String,
}

#[derive(Debug, Clone)]
struct FunctionRegion {
    path: String,
    name: String,
    start_line: usize,
    lines: Vec<String>,
}

impl FunctionRegion {
    fn contains(&self, needle: &str) -> bool {
        self.lines.iter().any(|line| line.contains(needle))
    }

    fn first_match(&self, predicate: impl Fn(&str) -> bool) -> Option<Match> {
        self.lines
            .iter()
            .enumerate()
            .find(|(_, line)| predicate(line))
            .map(|(offset, line)| Match {
                path: self.path.clone(),
                line: self.start_line + offset,
                excerpt: line.trim().to_owned(),
            })
    }
}

const SUPPRESSION_DIRECTIVE: &str = "ferric-lens: ignore-correctness-risks";

pub fn scan(sources: &[SourceFile]) -> CorrectnessScan {
    let mut scan = CorrectnessScan::default();
    detect_contextual_copy_risks(sources, &mut scan);

    let filtered = sources
        .iter()
        .filter(|source| {
            std::str::from_utf8(&source.bytes)
                .map(|text| !text.contains(SUPPRESSION_DIRECTIVE))
                .unwrap_or(true)
        })
        .cloned()
        .collect::<Vec<_>>();
    let sources = filtered.as_slice();

    detect_cargo_feature_resolution(sources, &mut scan);
    detect_symbolic_target_identity(sources, &mut scan);
    detect_stdout_mode_artifacts(sources, &mut scan);
    detect_workspace_manifest_snapshot_gap(sources, &mut scan);
    detect_lossy_git_paths(sources, &mut scan);
    scan.findings
        .sort_by(|a, b| (&a.rule, &a.subject).cmp(&(&b.rule, &b.subject)));
    scan.source_contexts.sort();
    scan.source_contexts.dedup();
    scan
}

fn detect_contextual_copy_risks(sources: &[SourceFile], scan: &mut CorrectnessScan) {
    const MUTATING_METHODS: [&str; 15] = [
        ".append(",
        ".clear(",
        ".dedup(",
        ".drain(",
        ".extend(",
        ".insert(",
        ".pop(",
        ".push(",
        ".remove(",
        ".retain(",
        ".sort(",
        ".sort_by(",
        ".sort_by_key(",
        ".truncate(",
        ".swap_remove(",
    ];

    for source in sources {
        let Ok(text) = std::str::from_utf8(&source.bytes) else {
            continue;
        };
        let lines = text.lines().collect::<Vec<_>>();
        let subject = if source.module_path.is_empty() {
            source.crate_name.clone()
        } else {
            format!("{}::{}", source.crate_name, source.module_path)
        };

        let iteration = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| {
                let trimmed = line.trim_start();
                trimmed.starts_with("for ")
                    && trimmed.contains(" in ")
                    && trimmed.contains(".clone()")
            })
            .map(|(index, line)| Match {
                path: source.relative_path.clone(),
                line: index + 1,
                excerpt: line.trim().to_owned(),
            })
            .collect::<Vec<_>>();

        if !iteration.is_empty() {
            push_finding(
                scan,
                "runtime.clone_for_iteration_candidate",
                &subject,
                EvidenceClass::Candidate,
                Priority::Observe,
                "A cloned value is used directly as a for-loop iterator",
                "inspect whether the loop can borrow or iterate the original collection; keep the clone when ownership or mutation semantics require it",
                vec![evidence("clone_for_iteration_sites", iteration.len())],
                vec![("clone_for_iteration_sites", iteration)],
            );
        }

        let mut clone_then_mutate = Vec::new();
        for (index, line) in lines.iter().enumerate() {
            let Some(name) = mutable_clone_binding(line) else {
                continue;
            };
            let end = (index + 13).min(lines.len());
            let prefix = format!("{name}.");
            let mutated = lines[index + 1..end].iter().any(|candidate| {
                let Some((_, after_root)) = candidate.split_once(&prefix) else {
                    return false;
                };
                after_root.contains('.')
                    && MUTATING_METHODS
                        .iter()
                        .any(|method| candidate.contains(method))
            });
            if mutated {
                clone_then_mutate.push(Match {
                    path: source.relative_path.clone(),
                    line: index + 1,
                    excerpt: line.trim().to_owned(),
                });
            }
        }

        if !clone_then_mutate.is_empty() {
            push_finding(
                scan,
                "runtime.clone_then_mutate_candidate",
                &subject,
                EvidenceClass::Candidate,
                Priority::Observe,
                "A cloned aggregate is followed by mutation of one of its nested fields",
                "inspect whether the operation can borrow the original aggregate and build only the filtered or changed subset instead of copying the whole value",
                vec![evidence(
                    "clone_then_mutate_sites",
                    clone_then_mutate.len(),
                )],
                vec![("clone_then_mutate_sites", clone_then_mutate)],
            );
        }
    }
}

fn mutable_clone_binding(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    let rest = trimmed.strip_prefix("let mut ")?;
    let (name, value) = rest.split_once('=')?;
    let name = name.trim();
    if name.is_empty() || !name.chars().all(is_ident_char) || !value.contains(".clone()") {
        return None;
    }
    Some(name)
}

fn detect_cargo_feature_resolution(sources: &[SourceFile], scan: &mut CorrectnessScan) {
    let no_deps = matches_where(sources, |line| line.contains("--no-deps"));
    let feature_branch = matches_where(sources, |line| {
        line.contains("\"feature\"") && line.contains("contains(")
    });
    if no_deps.is_empty() || feature_branch.is_empty() {
        return;
    }

    push_finding(
        scan,
        "correctness.cargo_feature_resolution_without_resolve",
        "repository",
        EvidenceClass::Strong,
        Priority::ActFirst,
        "Cargo feature cfg is inferred without Cargo's resolved per-package feature graph",
        "collect Cargo metadata with the resolve graph available, map enabled features per workspace package, and evaluate cfg(feature) against that package-specific resolved set; keep feature reachability unknown when resolution is unavailable",
        vec![
            evidence("cargo_metadata_no_deps_sites", no_deps.len()),
            evidence("cfg_feature_resolution_sites", feature_branch.len()),
        ],
        vec![
            ("cargo_metadata_no_deps_sites", no_deps),
            ("cfg_feature_resolution_sites", feature_branch),
        ],
    );
}

fn detect_symbolic_target_identity(sources: &[SourceFile], scan: &mut CorrectnessScan) {
    let resolved = matches_where(sources, |line| line.contains("resolved_target"));
    let symbolic = matches_where(sources, |line| {
        line.contains("format!") && line.contains("target={") && !line.contains("resolved_target")
    });
    let symbolic_match = matches_where(sources, |line| {
        line.contains("==") && line.contains(".target") && !line.contains(".resolved_target")
    });
    if symbolic.is_empty() || symbolic_match.is_empty() || resolved.is_empty() {
        return;
    }

    push_finding(
        scan,
        "correctness.symbolic_target_identity",
        "repository",
        EvidenceClass::Strong,
        Priority::ActFirst,
        "Machine configuration identity uses a symbolic target label instead of the resolved target",
        "use the resolved target triple in persistent finding/acceptance identity and in imported-evidence configuration matching; keep symbolic labels such as host only for display",
        vec![
            evidence("symbolic_target_identity_sites", symbolic.len()),
            evidence("symbolic_target_match_sites", symbolic_match.len()),
        ],
        vec![
            ("symbolic_target_identity_sites", symbolic),
            ("symbolic_target_match_sites", symbolic_match),
        ],
    );
}

fn detect_stdout_mode_artifacts(sources: &[SourceFile], scan: &mut CorrectnessScan) {
    const MODES: [&str; 5] = ["ai", "stdout", "compact", "machine", "json_only"];

    for source in sources {
        let Ok(text) = std::str::from_utf8(&source.bytes) else {
            continue;
        };
        let lines = text.lines().collect::<Vec<_>>();
        for (index, line) in lines.iter().enumerate() {
            if !(line.contains("print!(") || line.contains("println!(")) {
                continue;
            }
            let start = index.saturating_sub(14);
            let window = &lines[start..=index];
            let Some(mode) = MODES
                .iter()
                .find(|mode| window.iter().any(|line| contains_ident(line, mode)))
            else {
                continue;
            };
            let guarded = window
                .iter()
                .any(|line| line.contains("if") && contains_ident(line, mode));
            if guarded {
                continue;
            }
            let writes = window
                .iter()
                .enumerate()
                .filter(|(_, line)| {
                    line.contains("::write(")
                        || line.contains(".write(")
                        || line.contains("::create(")
                })
                .map(|(offset, line)| Match {
                    path: source.relative_path.clone(),
                    line: start + offset + 1,
                    excerpt: line.trim().to_owned(),
                })
                .collect::<Vec<_>>();
            if writes.len() < 2 {
                continue;
            }

            push_finding(
                scan,
                "correctness.stdout_mode_unconditional_artifacts",
                "repository",
                EvidenceClass::Strong,
                Priority::ActFirst,
                "A stdout-oriented output mode still performs unconditional default artifact writes",
                "make default artifact paths conditional on human/file output mode; in compact stdout/AI mode, write files only when the caller explicitly requested those output paths",
                vec![evidence("unconditional_output_write_sites", writes.len())],
                vec![("unconditional_output_write_sites", writes)],
            );
            return;
        }
    }
}

fn detect_workspace_manifest_snapshot_gap(sources: &[SourceFile], scan: &mut CorrectnessScan) {
    let functions = function_regions(sources);
    let root_only = functions
        .iter()
        .filter(|function| {
            function.contains("Cargo.toml")
                && function.contains("Cargo.lock")
                && !function.contains("manifest_path")
        })
        .collect::<Vec<_>>();
    let member_reads = functions
        .iter()
        .filter_map(|function| {
            if function.contains("manifest_path")
                && (function.contains("fs::read") || function.contains("read("))
            {
                function.first_match(|line| line.contains("manifest_path"))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();

    let mut verifier_refs = Vec::new();
    for verifier in functions.iter().filter(|function| {
        let name = function.name.to_ascii_lowercase();
        name.contains("verify") || name.contains("stable") || name.contains("check")
    }) {
        for root_function in &root_only {
            if let Some(found) =
                verifier.first_match(|line| line.contains(&format!("{}(", root_function.name)))
            {
                verifier_refs.push(found);
            }
        }
    }

    if root_only.is_empty() || member_reads.is_empty() || verifier_refs.is_empty() {
        return;
    }

    let root_matches = root_only
        .iter()
        .filter_map(|function| {
            function.first_match(|line| line.contains("Cargo.toml") && line.contains("Cargo.lock"))
        })
        .collect::<Vec<_>>();

    push_finding(
        scan,
        "correctness.workspace_manifest_snapshot_gap",
        "repository",
        EvidenceClass::Strong,
        Priority::ActFirst,
        "Snapshot stability verification does not cover all workspace manifests used during analysis",
        "capture the complete set of workspace-member manifests used by Cargo metadata, include them in the initial Cargo-input digest, and verify the same files again before publishing the snapshot",
        vec![
            evidence("root_only_cargo_input_sites", root_matches.len()),
            evidence("workspace_manifest_read_sites", member_reads.len()),
        ],
        vec![
            ("root_only_cargo_input_sites", root_matches),
            ("workspace_manifest_read_sites", member_reads),
        ],
    );
}

fn detect_lossy_git_paths(sources: &[SourceFile], scan: &mut CorrectnessScan) {
    let mut lossy = Vec::new();
    for source in sources {
        let Ok(text) = std::str::from_utf8(&source.bytes) else {
            continue;
        };
        let git_path_stream = text.contains("Command::new(\"git\")") && text.contains("\"-z\"");
        if !git_path_stream {
            continue;
        }
        for (index, line) in text.lines().enumerate() {
            let path_like = ["path", "record", "entry", "name"]
                .iter()
                .any(|word| contains_ident(line, word));
            if line.contains("from_utf8_lossy") && path_like {
                lossy.push(Match {
                    path: source.relative_path.clone(),
                    line: index + 1,
                    excerpt: line.trim().to_owned(),
                });
            }
        }
    }
    if lossy.is_empty() {
        return;
    }

    push_finding(
        scan,
        "correctness.lossy_git_path_decoding",
        "repository",
        EvidenceClass::Strong,
        Priority::ActFirst,
        "Git repository paths are decoded with a lossy UTF-8 conversion",
        "preserve Git path bytes/OS strings where possible; otherwise reject non-UTF-8 repository paths explicitly and mark the affected analysis capability incomplete instead of silently replacing bytes",
        vec![evidence("lossy_git_path_decode_sites", lossy.len())],
        vec![("lossy_git_path_decode_sites", lossy)],
    );
}

fn matches_where(sources: &[SourceFile], predicate: impl Fn(&str) -> bool) -> Vec<Match> {
    let mut out = Vec::new();
    for source in sources {
        let Ok(text) = std::str::from_utf8(&source.bytes) else {
            continue;
        };
        for (index, line) in text.lines().enumerate() {
            if predicate(line) {
                out.push(Match {
                    path: source.relative_path.clone(),
                    line: index + 1,
                    excerpt: line.trim().to_owned(),
                });
            }
        }
    }
    out
}

fn function_regions(sources: &[SourceFile]) -> Vec<FunctionRegion> {
    let mut out = Vec::new();
    for source in sources {
        let Ok(text) = std::str::from_utf8(&source.bytes) else {
            continue;
        };
        let Ok(file) = syn::parse_file(text) else {
            continue;
        };
        let source_lines = text.lines().collect::<Vec<_>>();
        for item in file.items {
            let Item::Fn(function) = item else {
                continue;
            };
            if function.attrs.iter().any(|attr| {
                attr.path().is_ident("cfg")
                    && attr
                        .parse_args::<syn::Path>()
                        .is_ok_and(|path| path.is_ident("test"))
            }) {
                continue;
            }
            let start = function.span().start().line.max(1);
            let end = function.span().end().line.max(start);
            let lines = source_lines
                .get(start - 1..end.min(source_lines.len()))
                .unwrap_or(&[])
                .iter()
                .map(|line| (*line).to_owned())
                .collect();
            out.push(FunctionRegion {
                path: source.relative_path.clone(),
                name: function.sig.ident.to_string(),
                start_line: start,
                lines,
            });
        }
    }
    out
}

fn contains_ident(line: &str, ident: &str) -> bool {
    line.match_indices(ident).any(|(start, _)| {
        let before = line[..start].chars().next_back();
        let after = line[start + ident.len()..].chars().next();
        !before.is_some_and(is_ident_char) && !after.is_some_and(is_ident_char)
    })
}

fn is_ident_char(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

fn evidence(metric: &str, value: usize) -> Evidence {
    Evidence {
        metric: metric.to_owned(),
        value,
        reference: 0,
        population: 1,
        baseline: None,
        material_delta: None,
    }
}

#[allow(clippy::too_many_arguments)]
fn push_finding(
    scan: &mut CorrectnessScan,
    rule: &str,
    subject: &str,
    evidence_class: EvidenceClass,
    priority: Priority,
    summary: &str,
    direction: &str,
    evidence: Vec<Evidence>,
    contexts: Vec<(&str, Vec<Match>)>,
) {
    scan.findings.push(Finding {
        fingerprint: String::new(),
        rule: rule.to_owned(),
        subject: subject.to_owned(),
        identity: subject.to_owned(),
        configuration: String::new(),
        evidence_class,
        priority,
        delta: DeltaStatus::Current,
        gate: false,
        accepted: false,
        acceptance_reason: None,
        summary: summary.to_owned(),
        direction: direction.to_owned(),
        evidence,
    });

    for (metric, matches) in contexts {
        for matched in matches.into_iter().take(3) {
            scan.source_contexts.push(SourceContext {
                subject: subject.to_owned(),
                metric: metric.to_owned(),
                path: matched.path,
                start_line: matched.line,
                end_line: matched.line,
                excerpt: matched.excerpt,
                excerpt_truncated: false,
            });
        }
    }
}

#[cfg(test)]
#[path = "correctness_tests.rs"]
mod tests;
