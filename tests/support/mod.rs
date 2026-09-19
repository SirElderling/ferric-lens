use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

static COUNTER: AtomicU64 = AtomicU64::new(0);

pub struct Repo {
    pub root: PathBuf,
}

impl Repo {
    pub fn new(name: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "ferric-lens-test-{name}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname='fixture'\nversion='0.1.0'\nedition='2021'\n",
        )
        .unwrap();
        git(&root, &["init", "-q"]);
        git(
            &root,
            &["config", "user.email", "ferric-lens@example.invalid"],
        );
        git(&root, &["config", "user.name", "Ferric Lens Test"]);
        Self { root }
    }

    pub fn baseline(name: &str) -> Self {
        let repo = Self::new(name);
        repo.write("src/lib.rs", "pub mod stable;\n");
        repo.write("src/stable.rs", "pub fn stable() -> usize { 1 }\n");
        repo.commit("baseline");
        repo
    }

    pub fn gate_fixture(name: &str) -> (Self, String) {
        let repo = Self::new(name);
        let declarations = (0..20)
            .map(|index| format!("mod m{index};"))
            .collect::<Vec<_>>()
            .join("\n");
        repo.write("src/lib.rs", &declarations);
        for index in 0..20 {
            let (decisions, dependencies) = match index {
                0 => (12, 4),
                1..=16 => (10, 3),
                _ => (15, 5),
            };
            repo.write(
                &format!("src/m{index}.rs"),
                &gate_module_source(index, decisions, dependencies),
            );
        }
        repo.commit("baseline");
        let baseline = git(&repo.root, &["rev-parse", "HEAD"]);
        repo.write("src/m0.rs", &gate_module_source(0, 18, 7));
        repo.commit("regression");
        (repo, baseline)
    }

    pub fn write(&self, path: &str, contents: &str) {
        let path = self.root.join(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    pub fn commit(&self, message: &str) {
        git(&self.root, &["add", "."]);
        git(&self.root, &["commit", "-q", "-m", message]);
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

pub fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn gate_module_source(index: usize, decisions: usize, dependencies: usize) -> String {
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
