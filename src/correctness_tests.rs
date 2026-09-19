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
    let output = Command::new("git").output()?;
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
