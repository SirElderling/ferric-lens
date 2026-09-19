use crate::{
    correctness::scan,
    input::SourceFile,
    model::{EvidenceClass, Priority},
};

fn source(path: &str, text: &str) -> SourceFile {
    SourceFile {
        crate_name: "demo".into(),
        module_path: path.trim_end_matches(".rs").replace('/', "::"),
        relative_path: path.into(),
        bytes: text.as_bytes().to_vec(),
    }
}

#[test]
fn detects_incomplete_cargo_feature_resolution() {
    let sources = vec![
        source(
            "src/input.rs",
            r#"
fn metadata() {
    command.args(["metadata", "--no-deps"]);
}
"#,
        ),
        source(
            "src/cfg.rs",
            r#"
fn evaluate(key: &str, explicit_features: &Set) {
    if key == "feature" && explicit_features.contains("fast") {}
}
"#,
        ),
    ];

    let scan = scan(&sources);
    let finding = scan
        .findings
        .iter()
        .find(|finding| finding.rule == "correctness.cargo_feature_resolution_without_resolve")
        .expect("feature-resolution risk");

    assert_eq!(finding.priority, Priority::ActFirst);
    assert_eq!(finding.evidence_class, EvidenceClass::Strong);
    assert!(finding.direction.contains("per workspace package"));
    assert!(scan
        .source_contexts
        .iter()
        .any(|context| context.path == "src/input.rs"));
    assert!(scan
        .source_contexts
        .iter()
        .any(|context| context.path == "src/cfg.rs"));
}

#[test]
fn detects_symbolic_host_identity_used_for_machine_matching() {
    let sources = vec![
        source(
            "src/profile.rs",
            r#"
let resolved_target = host()?;
let target_label = target.unwrap_or("host");
let id = format!("target={target_label};features=default");
"#,
        ),
        source(
            "src/evidence.rs",
            r#"
let configuration_matches = envelope.configuration.target == profile.target;
"#,
        ),
    ];

    let scan = scan(&sources);
    let finding = scan
        .findings
        .iter()
        .find(|finding| finding.rule == "correctness.symbolic_target_identity")
        .expect("target identity risk");

    assert!(finding.direction.contains("resolved target"));
    assert_eq!(finding.evidence.len(), 2);
}

#[test]
fn detects_ai_stdout_mode_with_unconditional_default_artifact_writes() {
    let sources = vec![source(
        "src/main.rs",
        r#"
Analyze { ai, json, html } => {
    let result = analyze()?;
    let json_text = report::json(&result);
    report::write(&json, &json_text)?;
    report::write(&html, &report::html(&result))?;
    print!("{}", output_text(&result, ai));
}
"#,
    )];

    let scan = scan(&sources);
    let finding = scan
        .findings
        .iter()
        .find(|finding| finding.rule == "correctness.stdout_mode_unconditional_artifacts")
        .expect("stdout side-effect risk");

    assert!(finding.direction.contains("explicitly requested"));
    assert_eq!(scan.source_contexts.len(), 2);
}

#[test]
fn does_not_flag_ai_output_writes_when_guarded() {
    let sources = vec![source(
        "src/main.rs",
        r#"
Analyze { ai, json, html } => {
    let result = analyze()?;
    if !ai {
        report::write(&json, &report::json(&result))?;
        report::write(&html, &report::html(&result))?;
    }
    print!("{}", output_text(&result, ai));
}
"#,
    )];

    assert!(!scan(&sources)
        .findings
        .iter()
        .any(|finding| finding.rule == "correctness.stdout_mode_unconditional_artifacts"));
}

#[test]
fn detects_workspace_manifest_snapshot_gap() {
    let sources = vec![source(
        "src/input.rs",
        r#"
fn cargo_input_digest(root: &Path) {
    for name in ["Cargo.toml", "Cargo.lock"] {}
}
fn cargo_resolution_identity(package: &Package) {
    let manifest = PathBuf::from(&package.manifest_path);
    let manifest_bytes = fs::read(&manifest)?;
}
fn verify_stable_inputs() {
    cargo_input_digest(root)?;
}
"#,
    )];

    let scan = scan(&sources);
    let finding = scan
        .findings
        .iter()
        .find(|finding| finding.rule == "correctness.workspace_manifest_snapshot_gap")
        .expect("workspace manifest snapshot risk");

    assert!(finding.direction.contains("workspace-member manifests"));
}

#[test]
fn detects_lossy_git_path_decoding() {
    let sources = vec![source(
        "src/git.rs",
        r#"
fn run() {
    let output = Command::new("git").args(["status", "-z"]).output()?;
    let record = String::from_utf8_lossy(record).into_owned();
    let path = String::from_utf8_lossy(path).into_owned();
}
"#,
    )];

    let scan = scan(&sources);
    let finding = scan
        .findings
        .iter()
        .find(|finding| finding.rule == "correctness.lossy_git_path_decoding")
        .expect("lossy Git path risk");

    assert!(finding.direction.contains("reject"));
    assert_eq!(finding.evidence[0].value, 2);
}

#[test]
fn clean_patterns_do_not_emit_correctness_findings() {
    let sources = vec![
        source(
            "src/input.rs",
            r#"
command.args(["metadata"]);
let cargo_inputs = workspace_member_manifests();
"#,
        ),
        source(
            "src/profile.rs",
            r#"let id = format!("resolved_target={resolved_target};features=default");"#,
        ),
        source(
            "src/git.rs",
            r#"
let path = std::str::from_utf8(path)
    .map_err(|_| "non-UTF-8 repository path")?;
"#,
        ),
    ];

    assert!(scan(&sources).findings.is_empty());
}

#[test]
fn invalid_utf8_sources_are_ignored_by_textual_correctness_detectors() {
    let invalid = SourceFile {
        crate_name: "demo".into(),
        module_path: "invalid".into(),
        relative_path: "src/invalid.rs".into(),
        bytes: vec![0xff, 0xfe],
    };

    assert!(scan(&[invalid]).findings.is_empty());
}

#[test]
fn one_unconditional_write_is_not_enough_for_stdout_side_effect_finding() {
    let sources = vec![source(
        "src/main.rs",
        r#"
Analyze { ai, json } => {
    report::write(&json, &report::json(&result))?;
    print!("{}", output_text(&result, ai));
}
"#,
    )];

    assert!(!scan(&sources)
        .findings
        .iter()
        .any(|finding| finding.rule == "correctness.stdout_mode_unconditional_artifacts"));
}

#[test]
fn multiple_correctness_findings_are_sorted_deterministically() {
    let sources = vec![
        source(
            "src/input.rs",
            r#"
fn metadata() {
    command.args(["metadata", "--no-deps"]);
}
fn cargo_input_digest() {
    for name in ["Cargo.toml", "Cargo.lock"] {}
}
fn cargo_resolution_identity(package: &Package) {
    let manifest = PathBuf::from(&package.manifest_path);
    let bytes = fs::read(&manifest)?;
}
fn verify_stable_inputs() {
    cargo_input_digest();
}
"#,
        ),
        source(
            "src/cfg.rs",
            r#"if key == "feature" { explicit_features.contains("fast"); }"#,
        ),
    ];

    let scan = scan(&sources);

    assert!(scan.findings.len() >= 2);
    assert!(scan
        .findings
        .windows(2)
        .all(|pair| (&pair[0].rule, &pair[0].subject) <= (&pair[1].rule, &pair[1].subject)));
}

#[test]
fn explicit_correctness_scan_suppression_prevents_self_or_generated_pattern_noise() {
    let sources = vec![source(
        "src/generated.rs",
        r#"
// ferric-lens: ignore-correctness-risks
fn generated() {
    let record = String::from_utf8_lossy(record);
    let output = Command::new("git").output();
}
"#,
    )];

    assert!(scan(&sources).findings.is_empty());
}

#[test]
fn correctness_detectors_are_not_tied_to_ferric_lens_variable_names() {
    let sources = vec![
        source(
            "src/cargo_probe.rs",
            r#"
fn inspect_manifest_graph() {
    command.args(["metadata", "--no-deps"]);
}
fn evaluate_cfg(selector: &str, enabled_options: &Set) {
    if selector == "feature" && enabled_options.contains("fast") {}
}
"#,
        ),
        source(
            "src/platform.rs",
            r#"
fn identity() {
    let resolved_target = resolve_platform();
    let display_target = requested.unwrap_or("host");
    let cache_key = format!("target={display_target};features=default");
}
fn compatible(left: &Config, right: &Config) -> bool {
    left.target == right.target
}
"#,
        ),
        source(
            "src/cli.rs",
            r#"
fn run(machine: bool) {
    let result = analyze();
    artifact::write(&json, serialize(&result));
    artifact::write(&html, render(&result));
    println!("{}", machine_output(&result, machine));
}
"#,
        ),
        source(
            "src/snapshot.rs",
            r#"
fn root_inputs(root: &Path) {
    for file in ["Cargo.toml", "Cargo.lock"] {}
}
fn member_state(pkg: &Package) {
    let manifest = pkg.manifest_path.clone();
    let bytes = fs::read(manifest)?;
}
fn check_snapshot(root: &Path) {
    root_inputs(root);
}
"#,
        ),
        source(
            "src/repository.rs",
            r#"
fn paths() {
    let bytes = Command::new("git").args(["ls-files", "-z"]).output()?;
    let entry = String::from_utf8_lossy(entry).into_owned();
}
"#,
        ),
    ];

    let result = scan(&sources);
    let rules = result
        .findings
        .iter()
        .map(|finding| finding.rule.as_str())
        .collect::<std::collections::BTreeSet<_>>();

    assert!(rules.contains("correctness.cargo_feature_resolution_without_resolve"));
    assert!(rules.contains("correctness.symbolic_target_identity"));
    assert!(rules.contains("correctness.stdout_mode_unconditional_artifacts"));
    assert!(rules.contains("correctness.workspace_manifest_snapshot_gap"));
    assert!(rules.contains("correctness.lossy_git_path_decoding"));
}

#[test]
fn workspace_snapshot_detector_does_not_flag_manifest_complete_capture_function() {
    let sources = vec![source(
        "src/snapshot.rs",
        r#"
fn cargo_inputs(root: &Path, package: &Package) {
    let mut paths = BTreeSet::from([root.join("Cargo.toml"), root.join("Cargo.lock")]);
    paths.insert(PathBuf::from(&package.manifest_path));
}
fn verify_snapshot() {
    cargo_inputs(root, package);
}
"#,
    )];

    assert!(!scan(&sources)
        .findings
        .iter()
        .any(|finding| finding.rule == "correctness.workspace_manifest_snapshot_gap"));
}

#[test]
fn lossy_non_path_git_text_does_not_trigger_path_risk_without_nul_path_stream() {
    let sources = vec![source(
        "src/git.rs",
        r#"
fn command_error() {
    let output = Command::new("git").arg("status").output()?;
    let message = String::from_utf8_lossy(&output.stderr);
}
"#,
    )];

    assert!(!scan(&sources)
        .findings
        .iter()
        .any(|finding| finding.rule == "correctness.lossy_git_path_decoding"));
}

#[test]
fn ordinary_printing_without_compact_mode_does_not_trigger_stdout_side_effect_risk() {
    let sources = vec![source(
        "src/cli.rs",
        r#"
fn run() {
    artifact::write(&json, serialize(&result));
    artifact::write(&html, render(&result));
    println!("{}", human_output(&result));
}
"#,
    )];

    assert!(!scan(&sources)
        .findings
        .iter()
        .any(|finding| finding.rule == "correctness.stdout_mode_unconditional_artifacts"));
}

#[test]
fn workspace_snapshot_detector_ignores_direct_cfg_test_compatibility_helpers() {
    let sources = vec![source(
        "src/input.rs",
        r#"
fn cargo_input_snapshot(package: &Package) {
    for name in ["Cargo.toml", "Cargo.lock"] {}
    let manifest = PathBuf::from(&package.manifest_path);
    let bytes = fs::read(&manifest).unwrap();
}

#[cfg(test)]
fn cargo_input_digest() {
    for name in ["Cargo.toml", "Cargo.lock"] {}
}

#[cfg(test)]
fn verify_stable_inputs() {
    cargo_input_digest();
}
"#,
    )];

    assert!(!scan(&sources)
        .findings
        .iter()
        .any(|finding| finding.rule == "correctness.workspace_manifest_snapshot_gap"));
}

#[test]
fn workspace_snapshot_detector_covers_nonmatching_and_matching_verifiers() {
    let sources = vec![source(
        "src/input.rs",
        r#"
fn cargo_input_digest() {
    for name in ["Cargo.toml", "Cargo.lock"] {}
}

fn cargo_resolution_identity(package: &Package) {
    let manifest = PathBuf::from(&package.manifest_path);
    let bytes = fs::read(&manifest).unwrap();
}

fn verify_other_state() {
    check_something_else();
}

fn verify_stable_inputs() {
    cargo_input_digest();
}
"#,
    )];

    assert!(scan(&sources)
        .findings
        .iter()
        .any(|finding| finding.rule == "correctness.workspace_manifest_snapshot_gap"));
}


#[test]
fn manifest_snapshot_detector_requires_verifier_to_reference_root_only_digest() {
    let sources = vec![source(
        "src/input.rs",
        r#"
fn cargo_input_digest(root: &Path) {
    for name in ["Cargo.toml", "Cargo.lock"] {}
}
fn cargo_resolution_identity(package: &Package) {
    let manifest = PathBuf::from(&package.manifest_path);
}
fn verify_stable_inputs() {
    verify_something_else();
}
"#,
    )];

    assert!(!scan(&sources)
        .findings
        .iter()
        .any(|finding| finding.rule == "correctness.workspace_manifest_snapshot_gap"));
}
