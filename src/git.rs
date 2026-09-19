use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

static WORKTREE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub struct GitState {
    pub head: Option<String>,
    pub dirty: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct BaselineSelection {
    pub target_ref: String,
    pub target_oid: String,
    pub merge_base: String,
}

#[derive(Debug, Clone, Default)]
pub struct ChangeSet {
    pub renames: BTreeMap<String, String>,
    pub added: BTreeSet<String>,
    pub deleted: BTreeSet<String>,
    pub modified: BTreeSet<String>,
}

#[derive(Debug, Clone)]
pub struct HistoryCommit {
    pub oid: String,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct HistorySample {
    pub commits: Vec<HistoryCommit>,
    pub changed_path_records: usize,
    pub broad_commits_excluded_from_cochange: usize,
    pub truncated: bool,
}

const HISTORY_MAX_COMMITS: usize = 2_000;
const HISTORY_MAX_PATH_RECORDS: usize = 100_000;
const HISTORY_MAX_COCHANGE_PATHS_PER_COMMIT: usize = 200;
const EXACT_RENAME_FALLBACK_LIMIT: usize = 1_000;

pub struct TemporaryWorktree {
    repo_root: PathBuf,
    path: PathBuf,
}

impl TemporaryWorktree {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TemporaryWorktree {
    fn drop(&mut self) {
        let _ = git_bytes(
            &self.repo_root,
            [
                OsStr::new("worktree"),
                OsStr::new("remove"),
                OsStr::new("--force"),
                self.path.as_os_str(),
            ],
        );
        let _ = fs::remove_dir_all(&self.path);
    }
}

pub fn inspect(root: &Path) -> GitState {
    let head = git_text(root, ["rev-parse", "HEAD"])
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());

    let dirty = git_bytes(
        root,
        [
            OsStr::new("status"),
            OsStr::new("--porcelain=v1"),
            OsStr::new("-z"),
            OsStr::new("--untracked-files=all"),
        ],
    )
    .ok()
    .map(|value| !value.is_empty());

    GitState { head, dirty }
}

pub fn resolve_baseline(root: &Path, explicit: Option<&str>) -> Result<BaselineSelection, String> {
    let head = git_text(root, ["rev-parse", "HEAD"])?.trim().to_owned();

    let candidates = if let Some(explicit) = explicit {
        vec![(explicit.to_owned(), true)]
    } else {
        automatic_target_candidates(root)
    };

    let mut selected = None;
    for (candidate, was_explicit) in candidates {
        if let Ok(target_oid) = resolve_commit(root, &candidate) {
            selected = Some((candidate, target_oid, was_explicit));
            break;
        }
    }

    let Some((target_ref, target_oid, was_explicit)) = selected else {
        return Err("no usable baseline target ref is available locally".into());
    };

    if target_oid == head && !was_explicit {
        return Err(format!(
            "automatically selected baseline {target_ref} resolves to HEAD; provide --base explicitly"
        ));
    }

    let merge_bases = git_text(
        root,
        ["merge-base", "--all", head.as_str(), target_oid.as_str()],
    )?;
    baseline_selection_from_output(target_ref, target_oid, &merge_bases)
}

fn baseline_selection_from_output(
    target_ref: String,
    target_oid: String,
    merge_bases: &str,
) -> Result<BaselineSelection, String> {
    let merge_base = parse_merge_base_output(&target_ref, merge_bases)?;
    Ok(BaselineSelection {
        target_ref,
        target_oid,
        merge_base,
    })
}

pub fn changes_since(root: &Path, merge_base: &str) -> Result<ChangeSet, String> {
    let output = git_bytes(
        root,
        [
            OsStr::new("diff"),
            OsStr::new("--name-status"),
            OsStr::new("-z"),
            OsStr::new("-M"),
            OsStr::new("--no-ext-diff"),
            OsStr::new(merge_base),
            OsStr::new("--"),
        ],
    )?;

    let records = output
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .map(|record| decode_git_text(record, "git diff path/status record"))
        .collect::<Result<Vec<_>, _>>()?;

    finalize_changes(root, merge_base, &records, list_untracked(root))
}

fn finalize_changes(
    root: &Path,
    merge_base: &str,
    records: &[String],
    untracked: Result<Vec<String>, String>,
) -> Result<ChangeSet, String> {
    let mut changes = parse_change_records(records)?;
    changes.added.extend(untracked?);
    detect_exact_worktree_renames(root, merge_base, &mut changes)?;
    Ok(changes)
}

fn detect_exact_worktree_renames(
    root: &Path,
    merge_base: &str,
    changes: &mut ChangeSet,
) -> Result<(), String> {
    if changes.deleted.is_empty()
        || changes.added.is_empty()
        || changes.deleted.len() > EXACT_RENAME_FALLBACK_LIMIT
        || changes.added.len() > EXACT_RENAME_FALLBACK_LIMIT
    {
        return Ok(());
    }

    let mut tree_args = vec![
        OsStr::new("ls-tree").to_os_string(),
        OsStr::new("-r").to_os_string(),
        OsStr::new("-z").to_os_string(),
        OsStr::new(merge_base).to_os_string(),
        OsStr::new("--").to_os_string(),
    ];
    tree_args.extend(changes.deleted.iter().map(OsString::from));
    let tree = git_bytes(root, tree_args)?;

    let deleted_by_oid = parse_tree_oids(&tree)?;

    let added_paths = changes.added.iter().cloned().collect::<Vec<_>>();
    let mut hash_args = vec![
        OsStr::new("hash-object").to_os_string(),
        OsStr::new("--").to_os_string(),
    ];
    hash_args.extend(added_paths.iter().map(OsString::from));
    let hashes = git_text_os(root, hash_args);
    apply_exact_rename_hashes(changes, deleted_by_oid, &added_paths, hashes)
}

fn apply_exact_rename_hashes(
    changes: &mut ChangeSet,
    deleted_by_oid: BTreeMap<String, Vec<String>>,
    added_paths: &[String],
    hashes: Result<String, String>,
) -> Result<(), String> {
    let hashes = hashes?;
    let added_by_oid = index_added_hashes(added_paths, &hashes)?;

    let mut exact = Vec::new();
    for (oid, deleted) in deleted_by_oid {
        let Some(added) = added_by_oid.get(&oid) else {
            continue;
        };
        if deleted.len() == 1 && added.len() == 1 {
            exact.push((deleted[0].clone(), added[0].clone()));
        }
    }

    for (old, new) in exact {
        changes.deleted.remove(&old);
        changes.added.remove(&new);
        changes.renames.insert(old, new);
    }

    Ok(())
}

fn parse_merge_base_output(target_ref: &str, output: &str) -> Result<String, String> {
    let merge_bases = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if merge_bases.len() != 1 {
        return Err(format!(
            "baseline target {target_ref} does not have exactly one usable merge base"
        ));
    }
    Ok(merge_bases[0].to_owned())
}

fn parse_change_records(records: &[String]) -> Result<ChangeSet, String> {
    let mut changes = ChangeSet::default();
    let mut index = 0;
    while index < records.len() {
        let status = &records[index];
        index += 1;

        if status.starts_with('R') {
            if index + 1 >= records.len() {
                return Err("git diff returned a truncated rename record".into());
            }
            let old = records[index].clone();
            let new = records[index + 1].clone();
            changes.renames.insert(old, new);
            index += 2;
            continue;
        }

        if index >= records.len() {
            return Err("git diff returned a truncated path record".into());
        }
        let path = records[index].clone();
        index += 1;

        match status.chars().next() {
            Some('A') => {
                changes.added.insert(path);
            }
            Some('D') => {
                changes.deleted.insert(path);
            }
            Some('M') | Some('T') => {
                changes.modified.insert(path);
            }
            _ => {}
        }
    }
    Ok(changes)
}

fn list_untracked(root: &Path) -> Result<Vec<String>, String> {
    let output = git_bytes(
        root,
        [
            OsStr::new("ls-files"),
            OsStr::new("--others"),
            OsStr::new("--exclude-standard"),
            OsStr::new("-z"),
            OsStr::new("--"),
        ],
    )?;
    output
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .map(|path| decode_git_text(path, "untracked Git path"))
        .collect()
}

fn parse_tree_oids(tree: &[u8]) -> Result<BTreeMap<String, Vec<String>>, String> {
    let mut by_oid = BTreeMap::<String, Vec<String>>::new();
    for record in tree
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
    {
        let text = decode_git_text(record, "git tree path record")?;
        let Some((metadata, path)) = text.split_once('\t') else {
            continue;
        };
        let Some(oid) = metadata.split_whitespace().nth(2) else {
            continue;
        };
        by_oid
            .entry(oid.to_owned())
            .or_default()
            .push(path.to_owned());
    }
    Ok(by_oid)
}

fn index_added_hashes(
    added_paths: &[String],
    hashes: &str,
) -> Result<BTreeMap<String, Vec<String>>, String> {
    let hashes = hashes.lines().map(str::trim).collect::<Vec<_>>();
    if hashes.len() != added_paths.len() {
        return Err("git hash-object returned an unexpected number of rename candidates".into());
    }

    let mut by_oid = BTreeMap::<String, Vec<String>>::new();
    for (path, oid) in added_paths.iter().cloned().zip(hashes) {
        by_oid.entry(oid.to_owned()).or_default().push(path);
    }
    Ok(by_oid)
}

pub fn sample_history(root: &Path) -> Result<HistorySample, String> {
    let output = git_bytes(
        root,
        [
            OsStr::new("log"),
            OsStr::new("--no-merges"),
            OsStr::new("--no-renames"),
            OsStr::new("--name-only"),
            OsStr::new("-z"),
            OsStr::new("--format=format:%x00%x00%H%x00"),
            OsStr::new("--max-count=2001"),
            OsStr::new("HEAD"),
            OsStr::new("--"),
        ],
    )?;

    parse_history(&output)
}

fn parse_history(output: &[u8]) -> Result<HistorySample, String> {
    let mut commits = Vec::new();
    let mut current: Option<HistoryCommit> = None;
    let mut changed_path_records = 0usize;
    let mut broad_commits = 0usize;
    let mut truncated = false;
    let mut empty_run = 0usize;

    for raw in output.split(|byte| *byte == 0) {
        if raw.is_empty() {
            empty_run += 1;
            continue;
        }

        if empty_run >= 2 {
            if current.is_some() {
                finish_history_commit(&mut commits, current.take(), &mut broad_commits);
                if commits.len() >= HISTORY_MAX_COMMITS {
                    truncated = true;
                    break;
                }
            }

            let oid = decode_git_text(raw, "git history commit identifier")?
                .trim()
                .to_owned();
            if !is_object_id(&oid) {
                return Err("git history stream contained an invalid commit identifier".into());
            }
            current = Some(HistoryCommit {
                oid,
                paths: Vec::new(),
            });
            empty_run = 0;
            continue;
        }

        empty_run = 0;
        let Some(commit) = current.as_mut() else {
            continue;
        };

        if changed_path_records >= HISTORY_MAX_PATH_RECORDS {
            truncated = true;
            break;
        }

        let raw_path = if commit.paths.is_empty() {
            raw.strip_prefix(b"\n").unwrap_or(raw)
        } else {
            raw
        };
        let path = decode_git_text(raw_path, "git history path")?;
        commit.paths.push(path);
        changed_path_records += 1;
    }

    if !truncated && current.is_some() {
        debug_assert!(commits.len() < HISTORY_MAX_COMMITS);
        finish_history_commit(&mut commits, current, &mut broad_commits);
    }

    Ok(HistorySample {
        commits,
        changed_path_records,
        broad_commits_excluded_from_cochange: broad_commits,
        truncated,
    })
}

fn finish_history_commit(
    commits: &mut Vec<HistoryCommit>,
    commit: Option<HistoryCommit>,
    broad_commits: &mut usize,
) {
    let Some(commit) = commit else {
        return;
    };
    if commit.paths.len() > HISTORY_MAX_COCHANGE_PATHS_PER_COMMIT {
        *broad_commits += 1;
    }
    commits.push(commit);
}

fn is_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub fn materialize_worktree(root: &Path, commit: &str) -> Result<TemporaryWorktree, String> {
    let counter = WORKTREE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "ferric-lens-baseline-{}-{counter}",
        std::process::id()
    ));
    prepare_worktree_path(&path)?;

    git_bytes(
        root,
        [
            OsStr::new("worktree"),
            OsStr::new("add"),
            OsStr::new("--detach"),
            OsStr::new("--quiet"),
            path.as_os_str(),
            OsStr::new(commit),
        ],
    )
    .map_err(|error| format!("cannot materialize baseline {commit}: {error}"))?;

    Ok(TemporaryWorktree {
        repo_root: root.to_path_buf(),
        path,
    })
}

fn prepare_worktree_path(path: &Path) -> Result<(), String> {
    if path.exists() {
        fs::remove_dir_all(path)
            .map_err(|error| format!("cannot clear temporary baseline directory: {error}"))?;
    }
    Ok(())
}

fn append_base_candidates(candidates: &mut Vec<(String, bool)>, base: &str) {
    let base = base.trim();
    if !base.is_empty() {
        candidates.push((format!("refs/remotes/origin/{base}"), false));
        candidates.push((base.to_owned(), false));
    }
}

fn automatic_target_candidates(root: &Path) -> Vec<(String, bool)> {
    let mut candidates = Vec::new();

    let base = std::env::var("GITHUB_BASE_REF").ok();
    append_optional_base_candidate(&mut candidates, base.as_deref());

    let symbolic = git_text(
        root,
        [
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    )
    .ok();
    append_symbolic_candidate(&mut candidates, symbolic.as_deref());

    candidates.push(("refs/remotes/origin/main".into(), false));
    candidates.push(("main".into(), false));

    candidates
}

fn append_optional_base_candidate(candidates: &mut Vec<(String, bool)>, base: Option<&str>) {
    if let Some(base) = base {
        append_base_candidates(candidates, base);
    }
}

fn append_symbolic_candidate(candidates: &mut Vec<(String, bool)>, symbolic: Option<&str>) {
    let Some(symbolic) = symbolic.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    candidates.push((symbolic.to_owned(), false));
}

fn resolve_commit(root: &Path, reference: &str) -> Result<String, String> {
    let spec = format!("{reference}^{{commit}}");
    let output = git_bytes(
        root,
        [
            OsStr::new("rev-parse"),
            OsStr::new("--verify"),
            OsStr::new("--end-of-options"),
            OsStr::new(&spec),
        ],
    )?;
    let output = decode_git_text(&output, "resolved Git commit")?;
    parse_resolved_commit(reference, &output)
}

fn parse_resolved_commit(reference: &str, output: &str) -> Result<String, String> {
    let value = output.trim().to_owned();
    if value.is_empty() {
        Err(format!("ref {reference} did not resolve to a commit"))
    } else {
        Ok(value)
    }
}

fn git_text<const N: usize>(root: &Path, args: [&str; N]) -> Result<String, String> {
    let args = args.map(OsStr::new);
    let output = git_bytes(root, args)?;
    decode_git_text(&output, "Git text output")
}

fn git_text_os(root: &Path, args: Vec<OsString>) -> Result<String, String> {
    let output = git_bytes(root, args)?;
    Ok(String::from_utf8_lossy(&output).into_owned())
}

fn decode_git_text(bytes: &[u8], context: &str) -> Result<String, String> {
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|_| format!("{context} is not valid UTF-8; repository path analysis is incomplete"))
}

fn git_bytes<I, S>(root: &Path, args: I) -> Result<Vec<u8>, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let args = args
        .into_iter()
        .map(|arg| arg.as_ref().to_os_string())
        .collect::<Vec<_>>();
    let output = Command::new("git")
        .current_dir(root)
        .args(&args)
        .output()
        .map_err(|error| format!("could not execute git: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        if stderr.is_empty() {
            return Err(format!("git command failed with status {}", output.status));
        }
        return Err(stderr);
    }
    Ok(output.stdout)
}

#[cfg(test)]
#[path = "git_tests.rs"]
mod tests;
