use std::{path::Path, process::Command};

#[derive(Debug, Clone)]
pub struct GitState {
    pub head: Option<String>,
    pub dirty: Option<bool>,
}

pub fn inspect(root: &Path) -> GitState {
    let head = command(root, &["rev-parse", "HEAD"])
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());

    let dirty = command(
        root,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )
    .ok()
    .map(|value| !value.is_empty());

    GitState { head, dirty }
}

fn command(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .map_err(|error| format!("could not execute git: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
