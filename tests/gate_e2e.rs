use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct FixtureRepo {
    root: PathBuf,
}

impl FixtureRepo {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "ferric-lens-e2e-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).unwrap();

        fs::write(
            root.join("Cargo.toml"),
            r#"[package]
name = "fixture"
version = "0.1.0"
edition = "2021"
"#,
        )
        .unwrap();
        fs::write(
            root.join("Cargo.lock"),
            r#"version = 4

[[package]]
name = "fixture"
version = "0.1.0"
"#,
        )
        .unwrap();

        git(&root, &["init", "-b", "main"]);
        git(&root, &["config", "user.name", "Ferric Lens Test"]);
        git(
            &root,
            &["config", "user.email", "ferric-lens@example.invalid"],
        );

        Self { root }
    }

    fn write_baseline(&self) {
        let declarations = (0..20)
            .map(|index| format!("mod m{index};"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(self.root.join("src/lib.rs"), declarations).unwrap();

        for index in 0..20 {
            let (decisions, dependencies) = match index {
                0 => (12, 4),
                1..=16 => (10, 3),
                _ => (15, 5),
            };
            fs::write(
                self.root.join(format!("src/m{index}.rs")),
                module_source(index, decisions, dependencies),
            )
            .unwrap();
        }

        git(&self.root, &["add", "."]);
        git(&self.root, &["commit", "-m", "baseline"]);
        git(&self.root, &["switch", "-c", "feature"]);
    }

    fn introduce_regression(&self) {
        fs::write(self.root.join("src/m0.rs"), module_source(0, 18, 7)).unwrap();
        git(&self.root, &["add", "src/m0.rs"]);
        git(&self.root, &["commit", "-m", "regression"]);
    }
}

impl Drop for FixtureRepo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn module_source(index: usize, decisions: usize, dependencies: usize) -> String {
    let mut source = String::new();
    for dependency in 1..=dependencies {
        let target = (index + dependency) % 20;
        if target != index {
            source.push_str(&format!("use crate::m{target};\n"));
        }
    }

    source.push_str("fn measured(value: bool) {\n");
    for _ in 0..decisions {
        source.push_str("    if value {}\n");
    }
    source.push_str("}\n");
    source
}

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn unchanged_branch_passes_without_reclassifying_existing_debt() {
    let fixture = FixtureRepo::new();
    fixture.write_baseline();

    let result = ferric_lens::check_with_base(&fixture.root, Some("main")).unwrap();

    assert_eq!(result.verdict, ferric_lens::model::GateVerdict::Pass);
    assert!(result.findings.iter().all(|finding| !finding.gate));
}

#[test]
fn material_two_signal_growth_is_a_regression() {
    let fixture = FixtureRepo::new();
    fixture.write_baseline();
    fixture.introduce_regression();

    let result = ferric_lens::check_with_base(&fixture.root, Some("main")).unwrap();

    assert_eq!(result.verdict, ferric_lens::model::GateVerdict::Regression);
    let gate = result
        .findings
        .iter()
        .find(|finding| finding.gate)
        .expect("expected gate regression");
    assert_eq!(gate.rule, "structure.coupled_complexity_growth");
    assert_eq!(gate.subject, "fixture::m0");
    assert!(!gate.accepted);
}

#[test]
fn exact_acceptance_keeps_finding_visible_but_allows_pass() {
    let fixture = FixtureRepo::new();
    fixture.write_baseline();
    fixture.introduce_regression();

    let first = ferric_lens::check_with_base(&fixture.root, Some("main")).unwrap();
    let fingerprint = first
        .findings
        .iter()
        .find(|finding| finding.gate)
        .unwrap()
        .fingerprint
        .clone();

    ferric_lens::accept_finding_with_base(
        &fixture.root,
        Some("main"),
        &fingerprint,
        "intentional fixture boundary",
    )
    .unwrap();

    let accepted = ferric_lens::check_with_base(&fixture.root, Some("main")).unwrap();

    assert_eq!(accepted.verdict, ferric_lens::model::GateVerdict::Pass);
    let finding = accepted
        .findings
        .iter()
        .find(|finding| finding.fingerprint == fingerprint)
        .unwrap();
    assert!(finding.gate);
    assert!(finding.accepted);
    assert_eq!(
        finding.acceptance_reason.as_deref(),
        Some("intentional fixture boundary")
    );
}
