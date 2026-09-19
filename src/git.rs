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
    let merge_bases = merge_bases
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();

    if merge_bases.len() != 1 {
        return Err(format!(
            "baseline target {target_ref} does not have exactly one usable merge base"
        ));
    }

    Ok(BaselineSelection {
        target_ref,
        target_oid,
        merge_base: merge_bases[0].to_owned(),
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
        .map(|record| String::from_utf8_lossy(record).into_owned())
        .collect::<Vec<_>>();

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

    let untracked = git_bytes(
        root,
        [
            OsStr::new("ls-files"),
            OsStr::new("--others"),
            OsStr::new("--exclude-standard"),
            OsStr::new("-z"),
            OsStr::new("--"),
        ],
    )?;
    for path in untracked
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
    {
        changes
            .added
            .insert(String::from_utf8_lossy(path).into_owned());
    }

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

    let mut deleted_by_oid = BTreeMap::<String, Vec<String>>::new();
    for record in tree.split(|byte| *byte == 0).filter(|record| !record.is_empty()) {
        let text = String::from_utf8_lossy(record);
        let Some((metadata, path)) = text.split_once('\\t') else {
            continue;
        };
        let Some(oid) = metadata.split_whitespace().nth(2) else {
            continue;
        };
        deleted_by_oid
            .entry(oid.to_owned())
            .or_default()
            .push(path.to_owned());
    }

    let added_paths = changes.added.iter().cloned().collect::<Vec<_>>();
    let mut hash_args = vec![
        OsStr::new("hash-object").to_os_string(),
        OsStr::new("--").to_os_string(),
    ];
    hash_args.extend(added_paths.iter().map(OsString::from));
    let hashes = git_text_os(root, hash_args)?;
    let hashes = hashes.lines().map(str::trim).collect::<Vec<_>>();
    if hashes.len() != added_paths.len() {
        return Err("git hash-object returned an unexpected number of rename candidates".into());
    }

    let mut added_by_oid = BTreeMap::<String, Vec<String>>::new();
    for (path, oid) in added_paths.into_iter().zip(hashes) {
        added_by_oid.entry(oid.to_owned()).or_default().push(path);
    }

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
            if let Some(commit) = current.take() {
                if commit.paths.len() > HISTORY_MAX_COCHANGE_PATHS_PER_COMMIT {
                    broad_commits += 1;
                }
                commits.push(commit);
                if commits.len() >= HISTORY_MAX_COMMITS {
                    truncated = true;
                    break;
                }
            }

            let oid = String::from_utf8_lossy(raw).trim().to_owned();
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

        let path = String::from_utf8_lossy(raw).into_owned();
        if !path.is_empty() {
            commit.paths.push(path);
            changed_path_records += 1;
        }
    }

    if !truncated {
        if let Some(commit) = current {
            if commits.len() < HISTORY_MAX_COMMITS {
                if commit.paths.len() > HISTORY_MAX_COCHANGE_PATHS_PER_COMMIT {
                    broad_commits += 1;
                }
                commits.push(commit);
            } else {
                truncated = true;
            }
        }
    }

    Ok(HistorySample {
        commits,
        changed_path_records,
        broad_commits_excluded_from_cochange: broad_commits,
        truncated,
    })
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
    if path.exists() {
        fs::remove_dir_all(&path)
            .map_err(|error| format!("cannot clear temporary baseline directory: {error}"))?;
    }

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

fn automatic_target_candidates(root: &Path) -> Vec<(String, bool)> {
    let mut candidates = Vec::new();

    if let Ok(base) = std::env::var("GITHUB_BASE_REF") {
        let base = base.trim();
        if !base.is_empty() {
            candidates.push((format!("refs/remotes/origin/{base}"), false));
            candidates.push((base.to_owned(), false));
        }
    }

    if let Ok(symbolic) = git_text(
        root,
        [
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    ) {
        let symbolic = symbolic.trim();
        if !symbolic.is_empty() {
            candidates.push((symbolic.to_owned(), false));
        }
    }

    candidates.push(("refs/remotes/origin/main".into(), false));
    candidates.push(("main".into(), false));

    candidates
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
    let value = String::from_utf8_lossy(&output).trim().to_owned();
    if value.is_empty() {
        Err(format!("ref {reference} did not resolve to a commit"))
    } else {
        Ok(value)
    }
}

fn git_text<const N: usize>(root: &Path, args: [&str; N]) -> Result<String, String> {
    let args = args.map(OsStr::new);
    let output = git_bytes(root, args)?;
    Ok(String::from_utf8_lossy(&output).into_owned())
}

fn git_text_os(root: &Path, args: Vec<OsString>) -> Result<String, String> {
    let output = git_bytes(root, args)?;
    Ok(String::from_utf8_lossy(&output).into_owned())
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
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        process::Command,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::{
        changes_since, inspect, is_object_id, materialize_worktree, parse_history,
        resolve_baseline, resolve_commit, sample_history, ChangeSet,
    };

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct Repo {
        root: PathBuf,
    }

    impl Repo {
        fn new(name: &str) -> Self {
            let counter = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "ferric-lens-git-test-{}-{counter}-{name}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            git(&root, &["init", "-q", "-b", "main"]);
            git(&root, &["config", "user.email", "test@example.invalid"]);
            git(&root, &["config", "user.name", "Ferric Lens Test"]);
            Self { root }
        }

        fn write(&self, path: &str, contents: &str) {
            let path = self.root.join(path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(path, contents).unwrap();
        }

        fn commit(&self, message: &str) -> String {
            git(&self.root, &["add", "-A"]);
            git(&self.root, &["commit", "-q", "-m", message]);
            git(&self.root, &["rev-parse", "HEAD"])
        }
    }

    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn git(root: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(root)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    #[test]
    fn change_set_defaults_empty() {
        let changes = ChangeSet::default();
        assert!(changes.renames.is_empty());
        assert!(changes.added.is_empty());
        assert!(changes.deleted.is_empty());
        assert!(changes.modified.is_empty());
    }

    #[test]
    fn parses_nul_delimited_history_records() {
        let first = "1111111111111111111111111111111111111111";
        let second = "2222222222222222222222222222222222222222";
        let bytes = [
            b"\0\0".as_slice(),
            first.as_bytes(),
            b"\0src/a.rs\0src/b.rs\0\0\0".as_slice(),
            second.as_bytes(),
            b"\0src/a.rs\0".as_slice(),
        ]
        .concat();

        let sample = parse_history(&bytes).unwrap();
        assert_eq!(sample.commits.len(), 2);
        assert_eq!(sample.changed_path_records, 3);
        assert_eq!(sample.commits[0].paths, ["src/a.rs", "src/b.rs"]);
        assert_eq!(sample.commits[1].paths, ["src/a.rs"]);
    }

    #[test]
    fn samples_real_checkout_when_git_metadata_is_available() {
        if !Path::new(".git").exists() {
            return;
        }

        let sample = sample_history(Path::new(".")).unwrap();
        assert!(!sample.commits.is_empty());
        assert!(sample
            .commits
            .iter()
            .all(|commit| matches!(commit.oid.len(), 40 | 64)));
    }

    #[test]
    fn inspect_reports_clean_dirty_and_non_repository_states() {
        let repo = Repo::new("inspect");
        repo.write("tracked.txt", "one");
        let head = repo.commit("initial");

        let clean = inspect(&repo.root);
        assert_eq!(clean.head.as_deref(), Some(head.as_str()));
        assert_eq!(clean.dirty, Some(false));

        repo.write("untracked.txt", "new");
        let dirty = inspect(&repo.root);
        assert_eq!(dirty.head.as_deref(), Some(head.as_str()));
        assert_eq!(dirty.dirty, Some(true));

        let outside = std::env::temp_dir().join(format!(
            "ferric-lens-not-git-{}-{}",
            std::process::id(),
            TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&outside);
        fs::create_dir_all(&outside).unwrap();
        let unknown = inspect(&outside);
        assert!(unknown.head.is_none());
        assert!(unknown.dirty.is_none());
        fs::remove_dir_all(outside).unwrap();
    }

    #[test]
    fn explicit_baseline_resolves_and_temporary_worktree_is_removed_on_drop() {
        let repo = Repo::new("baseline");
        repo.write("tracked.txt", "one");
        let first = repo.commit("first");
        repo.write("tracked.txt", "two");
        repo.commit("second");

        let selection = resolve_baseline(&repo.root, Some(&first)).unwrap();
        assert_eq!(selection.target_oid, first);
        assert_eq!(selection.merge_base, first);

        let path = {
            let worktree = materialize_worktree(&repo.root, &selection.merge_base).unwrap();
            let path = worktree.path().to_path_buf();
            assert!(path.exists());
            assert_eq!(fs::read_to_string(path.join("tracked.txt")).unwrap(), "one");
            path
        };
        assert!(!path.exists());
    }

    #[test]
    fn automatic_baseline_rejects_main_when_it_is_head() {
        let repo = Repo::new("automatic-head");
        repo.write("tracked.txt", "one");
        repo.commit("first");

        let error = resolve_baseline(&repo.root, None).unwrap_err();

        assert!(error.contains("resolves to HEAD"));
        assert!(error.contains("provide --base explicitly"));
    }

    #[test]
    fn baseline_reports_missing_target_and_unrelated_histories() {
        let repo = Repo::new("missing-base");
        repo.write("tracked.txt", "one");
        repo.commit("first");
        git(&repo.root, &["checkout", "-q", "-b", "feature"]);
        git(&repo.root, &["branch", "-D", "main"]);

        assert!(resolve_baseline(&repo.root, None)
            .unwrap_err()
            .contains("no usable baseline target"));

        let repo = Repo::new("unrelated");
        repo.write("tracked.txt", "one");
        let main = repo.commit("main");
        git(&repo.root, &["checkout", "-q", "--orphan", "orphan"]);
        git(&repo.root, &["rm", "-q", "-rf", "."]);
        repo.write("other.txt", "other");
        repo.commit("orphan");

        let error = resolve_baseline(&repo.root, Some(&main)).unwrap_err();
        assert!(!error.is_empty());
    }

    #[test]
    fn changes_since_classifies_add_delete_modify_and_rename() {
        let repo = Repo::new("changes");
        repo.write("delete.rs", "delete");
        repo.write("modify.rs", "before");
        repo.write("rename.rs", "same content");
        let baseline = repo.commit("baseline");

        fs::remove_file(repo.root.join("delete.rs")).unwrap();
        repo.write("modify.rs", "after");
        fs::rename(repo.root.join("rename.rs"), repo.root.join("renamed.rs")).unwrap();
        repo.write("added.rs", "added");

        let changes = changes_since(&repo.root, &baseline).unwrap();

        assert!(changes.added.contains("added.rs"));
        assert!(changes.deleted.contains("delete.rs"));
        assert!(changes.modified.contains("modify.rs"));
        assert_eq!(
            changes.renames.get("rename.rs").map(String::as_str),
            Some("renamed.rs")
        );
    }

    #[test]
    fn git_operations_report_invalid_repositories_and_refs() {
        let outside = std::env::temp_dir().join(format!(
            "ferric-lens-git-errors-{}-{}",
            std::process::id(),
            TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&outside);
        fs::create_dir_all(&outside).unwrap();

        assert!(resolve_baseline(&outside, Some("HEAD")).is_err());
        assert!(changes_since(&outside, "HEAD").is_err());
        assert!(sample_history(&outside).is_err());
        assert!(materialize_worktree(&outside, "HEAD").is_err());

        let repo = Repo::new("invalid-ref");
        repo.write("tracked.txt", "one");
        repo.commit("first");
        assert!(resolve_commit(&repo.root, "definitely-missing").is_err());

        fs::remove_dir_all(outside).unwrap();
    }

    #[test]
    fn history_rejects_invalid_ids_and_counts_broad_commits() {
        let invalid = b"\0\0not-an-object-id\0path.rs\0";
        assert!(parse_history(invalid)
            .unwrap_err()
            .contains("invalid commit identifier"));

        let oid = "a".repeat(40);
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"\0\0");
        bytes.extend_from_slice(oid.as_bytes());
        bytes.push(0);
        for index in 0..=super::HISTORY_MAX_COCHANGE_PATHS_PER_COMMIT {
            bytes.extend_from_slice(format!("src/{index}.rs").as_bytes());
            bytes.push(0);
        }

        let sample = parse_history(&bytes).unwrap();
        assert_eq!(sample.commits.len(), 1);
        assert_eq!(sample.broad_commits_excluded_from_cochange, 1);
    }

    #[test]
    fn history_ignores_records_before_first_commit_and_honors_commit_limit() {
        let mut bytes = b"orphan-path\0".to_vec();
        for index in 0..=super::HISTORY_MAX_COMMITS {
            bytes.extend_from_slice(b"\0\0");
            let oid = format!("{index:040x}");
            bytes.extend_from_slice(oid.as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(b"src/a.rs\0");
        }

        let sample = parse_history(&bytes).unwrap();

        assert_eq!(sample.commits.len(), super::HISTORY_MAX_COMMITS);
        assert!(sample.truncated);
        assert_eq!(sample.changed_path_records, super::HISTORY_MAX_COMMITS);
    }

    #[test]
    fn history_honors_changed_path_record_limit() {
        let oid = "b".repeat(40);
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"\0\0");
        bytes.extend_from_slice(oid.as_bytes());
        bytes.push(0);
        for index in 0..=super::HISTORY_MAX_PATH_RECORDS {
            bytes.extend_from_slice(format!("src/{index}.rs").as_bytes());
            bytes.push(0);
        }

        let sample = parse_history(&bytes).unwrap();

        assert!(sample.truncated);
        assert_eq!(sample.changed_path_records, super::HISTORY_MAX_PATH_RECORDS);
    }

    #[test]
    fn object_id_validation_accepts_sha1_and_sha256_only() {
        assert!(is_object_id(&"a".repeat(40)));
        assert!(is_object_id(&"B".repeat(64)));
        assert!(!is_object_id(&"a".repeat(39)));
        assert!(!is_object_id(&format!("{}z", "a".repeat(39))));
    }

    #[test]
    fn automatic_candidates_include_symbolic_origin_head() {
        let repo = Repo::new("origin-head");
        repo.write("tracked.txt", "one");
        let head = repo.commit("first");
        git(
            &repo.root,
            &["update-ref", "refs/remotes/origin/main", &head],
        );
        git(
            &repo.root,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main",
            ],
        );

        let candidates = super::automatic_target_candidates(&repo.root);

        assert!(candidates
            .iter()
            .any(|(candidate, explicit)| candidate == "origin/main" && !explicit));
    }
}
