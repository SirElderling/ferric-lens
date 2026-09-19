use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

use super::{
    changes_since, inspect, is_object_id, materialize_worktree, parse_history, resolve_baseline,
    resolve_commit, sample_history, ChangeSet,
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

#[test]
fn merge_base_parser_requires_exactly_one_commit() {
    assert!(super::parse_merge_base_output("main", "").is_err());
    assert!(super::parse_merge_base_output("main", "a\nb\n").is_err());
    assert_eq!(
        super::parse_merge_base_output("main", "  abc123  \n").unwrap(),
        "abc123"
    );
}

#[test]
fn change_record_parser_covers_all_supported_statuses_and_truncation() {
    let records = vec![
        "A".to_owned(),
        "added.rs".to_owned(),
        "D".to_owned(),
        "deleted.rs".to_owned(),
        "M".to_owned(),
        "modified.rs".to_owned(),
        "T".to_owned(),
        "typed.rs".to_owned(),
        "X".to_owned(),
        "ignored.rs".to_owned(),
        "R100".to_owned(),
        "old.rs".to_owned(),
        "new.rs".to_owned(),
    ];
    let changes = super::parse_change_records(&records).unwrap();

    assert!(changes.added.contains("added.rs"));
    assert!(changes.deleted.contains("deleted.rs"));
    assert!(changes.modified.contains("modified.rs"));
    assert!(changes.modified.contains("typed.rs"));
    assert!(!changes.modified.contains("ignored.rs"));
    assert_eq!(
        changes.renames.get("old.rs").map(String::as_str),
        Some("new.rs")
    );

    assert!(super::parse_change_records(&["M".to_owned()]).is_err());
    assert!(super::parse_change_records(&["R100".to_owned(), "old.rs".to_owned()]).is_err());
}

#[test]
fn exact_rename_helpers_reject_malformed_or_mismatched_git_output() {
    let tree = b"100644 blob abc123\told.rs\x00malformed\x00100644 blob\tmissing.rs\x00";
    let parsed = super::parse_tree_oids(tree);
    assert_eq!(
        parsed
            .get("abc123")
            .and_then(|paths| paths.first())
            .map(String::as_str),
        Some("old.rs")
    );

    let added = vec!["new.rs".to_owned(), "other.rs".to_owned()];
    assert!(super::index_added_hashes(&added, "abc123\n").is_err());
    let indexed = super::index_added_hashes(&added, "abc123\ndef456\n").unwrap();
    assert_eq!(indexed["abc123"], ["new.rs"]);
    assert_eq!(indexed["def456"], ["other.rs"]);
}

#[test]
fn baseline_candidate_and_resolved_commit_parsers_handle_empty_inputs() {
    let mut candidates = Vec::new();
    super::append_base_candidates(&mut candidates, " main ");
    super::append_base_candidates(&mut candidates, "   ");
    assert_eq!(
        candidates,
        vec![
            ("refs/remotes/origin/main".to_owned(), false),
            ("main".to_owned(), false)
        ]
    );

    assert!(super::parse_resolved_commit("main", "   ").is_err());
    assert_eq!(
        super::parse_resolved_commit("main", "abc123\n").unwrap(),
        "abc123"
    );
}

#[test]
fn worktree_path_preparation_removes_stale_directory() {
    let root = std::env::temp_dir().join(format!(
        "ferric-lens-worktree-prepare-{}-{}",
        std::process::id(),
        TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("stale"), "old").unwrap();

    super::prepare_worktree_path(&root).unwrap();

    assert!(!root.exists());
}

#[test]
fn history_counts_broad_commit_when_a_following_commit_starts() {
    let first = "a".repeat(40);
    let second = "b".repeat(40);
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"\0\0");
    bytes.extend_from_slice(first.as_bytes());
    bytes.push(0);
    for index in 0..=super::HISTORY_MAX_COCHANGE_PATHS_PER_COMMIT {
        bytes.extend_from_slice(format!("src/{index}.rs").as_bytes());
        bytes.push(0);
    }
    bytes.extend_from_slice(b"\0\0");
    bytes.extend_from_slice(second.as_bytes());
    bytes.extend_from_slice(b"\0src/final.rs\0");

    let sample = parse_history(&bytes).unwrap();

    assert_eq!(sample.commits.len(), 2);
    assert_eq!(sample.broad_commits_excluded_from_cochange, 1);
}

#[test]
fn staged_rename_is_classified_by_native_git_diff() {
    let repo = Repo::new("staged-rename");
    repo.write("old.rs", "same");
    let baseline = repo.commit("baseline");
    git(&repo.root, &["mv", "old.rs", "new.rs"]);

    let changes = changes_since(&repo.root, &baseline).unwrap();

    assert_eq!(
        changes.renames.get("old.rs").map(String::as_str),
        Some("new.rs")
    );
}

#[test]
fn untracked_listing_reports_non_repository_errors() {
    let root = std::env::temp_dir().join(format!(
        "ferric-lens-untracked-error-{}-{}",
        std::process::id(),
        TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&root).unwrap();

    assert!(super::list_untracked(&root).is_err());

    fs::remove_dir_all(root).unwrap();
}
#[test]
fn final_history_commit_is_finished_through_the_shared_helper() {
    let mut commits = Vec::new();
    let mut broad = 0;
    let commit = super::HistoryCommit {
        oid: "a".repeat(40),
        paths: vec!["src/a.rs".into()],
    };

    super::finish_history_commit(&mut commits, Some(commit), &mut broad);

    assert_eq!(commits.len(), 1);
    assert_eq!(broad, 0);

    let broad_commit = super::HistoryCommit {
        oid: "b".repeat(40),
        paths: (0..=super::HISTORY_MAX_COCHANGE_PATHS_PER_COMMIT)
            .map(|index| format!("src/{index}.rs"))
            .collect(),
    };
    super::finish_history_commit(&mut commits, Some(broad_commit), &mut broad);
    super::finish_history_commit(&mut commits, None, &mut broad);

    assert_eq!(commits.len(), 2);
    assert_eq!(broad, 1);
}

#[test]
fn worktree_path_preparation_reports_non_directory_removal_errors() {
    let root = std::env::temp_dir().join(format!(
        "ferric-lens-worktree-file-{}-{}",
        std::process::id(),
        TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(&root, "file").unwrap();

    let error = super::prepare_worktree_path(&root).unwrap_err();

    assert!(error.contains("cannot clear temporary baseline directory"));
    fs::remove_file(root).unwrap();
}

#[test]
fn git_execution_reports_missing_working_directory() {
    let root = std::env::temp_dir().join(format!(
        "ferric-lens-missing-cwd-{}-{}",
        std::process::id(),
        TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&root);

    let error = super::git_bytes(&root, [std::ffi::OsStr::new("status")]).unwrap_err();

    assert!(error.contains("could not execute git"));
}

#[test]
fn baseline_selection_helper_propagates_merge_base_parse_failures() {
    assert!(
        super::baseline_selection_from_output("main".into(), "a".repeat(40), "")
            .unwrap_err()
            .contains("exactly one usable merge base")
    );

    let selection = super::baseline_selection_from_output(
        "main".into(),
        "a".repeat(40),
        "b".repeat(40).as_str(),
    )
    .unwrap();
    assert_eq!(selection.target_ref, "main");
}

#[test]
fn finalize_changes_propagates_parse_untracked_and_rename_failures() {
    let repo = Repo::new("finalize-changes-errors");

    assert!(
        super::finalize_changes(&repo.root, "HEAD", &["M".into()], Ok(Vec::new()),)
            .unwrap_err()
            .contains("truncated path record")
    );

    assert_eq!(
        super::finalize_changes(&repo.root, "HEAD", &[], Err("untracked failed".into()))
            .unwrap_err(),
        "untracked failed"
    );

    let records = vec!["D".into(), "old.rs".into(), "A".into(), "new.rs".into()];
    assert!(
        super::finalize_changes(&repo.root, "definitely-missing", &records, Ok(Vec::new()),)
            .is_err()
    );
}

#[test]
fn exact_rename_detection_propagates_hash_errors_and_ambiguous_hashes() {
    let repo = Repo::new("rename-hash-errors");
    repo.write("old.rs", "same");
    let baseline = repo.commit("baseline");

    let mut missing_added = ChangeSet::default();
    missing_added.deleted.insert("old.rs".into());
    missing_added.added.insert("missing.rs".into());
    assert!(
        super::detect_exact_worktree_renames(&repo.root, &baseline, &mut missing_added,).is_err()
    );

    let mut ambiguous = ChangeSet::default();
    ambiguous
        .deleted
        .extend(["old1.rs".into(), "old2.rs".into()]);
    ambiguous.added.extend(["new1.rs".into(), "new2.rs".into()]);
    let deleted_by_oid = std::collections::BTreeMap::from([(
        "same".into(),
        vec!["old1.rs".into(), "old2.rs".into()],
    )]);
    super::apply_exact_rename_hashes(
        &mut ambiguous,
        deleted_by_oid,
        &["new1.rs".into(), "new2.rs".into()],
        Ok("same\nsame\n".into()),
    )
    .unwrap();
    assert!(ambiguous.renames.is_empty());

    let mut mismatch = ChangeSet::default();
    assert!(super::apply_exact_rename_hashes(
        &mut mismatch,
        std::collections::BTreeMap::new(),
        &["new.rs".into()],
        Ok(String::new()),
    )
    .is_err());

    assert_eq!(
        super::apply_exact_rename_hashes(
            &mut mismatch,
            std::collections::BTreeMap::new(),
            &[],
            Err("hash failed".into()),
        )
        .unwrap_err(),
        "hash failed"
    );
}

#[test]
fn automatic_candidate_helpers_cover_absent_empty_and_present_values() {
    let mut candidates = Vec::new();
    super::append_optional_base_candidate(&mut candidates, None);
    super::append_optional_base_candidate(&mut candidates, Some(" "));
    super::append_optional_base_candidate(&mut candidates, Some("develop"));
    super::append_symbolic_candidate(&mut candidates, None);
    super::append_symbolic_candidate(&mut candidates, Some(" "));
    super::append_symbolic_candidate(&mut candidates, Some("origin/trunk"));

    assert!(candidates
        .iter()
        .any(|(candidate, _)| candidate == "refs/remotes/origin/develop"));
    assert!(candidates
        .iter()
        .any(|(candidate, _)| candidate == "develop"));
    assert!(candidates
        .iter()
        .any(|(candidate, _)| candidate == "origin/trunk"));
}

#[test]
fn git_text_os_propagates_command_failures() {
    use std::ffi::OsString;

    let repo = Repo::new("git-text-os-error");
    let error = super::git_text_os(
        &repo.root,
        vec![OsString::from("definitely-not-a-git-subcommand")],
    )
    .unwrap_err();
    assert!(!error.is_empty());
}

#[test]
fn materialize_worktree_reports_stale_non_directory_path() {
    let repo = Repo::new("worktree-path-error");
    let head = git(&repo.root, &["rev-parse", "HEAD"]);
    let counter = 900_000 + TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    super::WORKTREE_COUNTER.store(counter, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "ferric-lens-baseline-{}-{counter}",
        std::process::id()
    ));
    let _ = fs::remove_file(&path);
    let _ = fs::remove_dir_all(&path);
    fs::write(&path, "blocker").unwrap();

    let error = super::materialize_worktree(&repo.root, &head).err().unwrap();

    assert!(error.contains("cannot clear temporary baseline directory"));
    fs::remove_file(path).unwrap();
}

#[test]
fn baseline_selection_helper_propagates_merge_base_shape_errors() {
    assert!(super::baseline_selection_from_output("main".into(), "a".repeat(40), "").is_err());
}

#[test]
fn exact_rename_detection_reports_tree_lookup_failure() {
    let root = std::env::temp_dir().join(format!(
        "ferric-lens-rename-tree-error-{}-{}",
        std::process::id(),
        TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&root).unwrap();
    let mut changes = ChangeSet::default();
    changes.deleted.insert("old.rs".into());
    changes.added.insert("new.rs".into());

    assert!(super::detect_exact_worktree_renames(&root, "HEAD", &mut changes).is_err());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn exact_rename_detection_reports_missing_added_path_hash_failure() {
    let repo = Repo::new("rename-hash-error");
    repo.write("old.rs", "same");
    let baseline = repo.commit("baseline");
    fs::remove_file(repo.root.join("old.rs")).unwrap();

    let mut changes = ChangeSet::default();
    changes.deleted.insert("old.rs".into());
    changes.added.insert("missing-new.rs".into());

    assert!(super::detect_exact_worktree_renames(&repo.root, &baseline, &mut changes).is_err());
}

#[test]
fn exact_rename_detection_does_not_guess_ambiguous_duplicate_content() {
    let repo = Repo::new("rename-ambiguous");
    repo.write("old-a.rs", "same");
    repo.write("old-b.rs", "same");
    let baseline = repo.commit("baseline");
    fs::remove_file(repo.root.join("old-a.rs")).unwrap();
    fs::remove_file(repo.root.join("old-b.rs")).unwrap();
    repo.write("new.rs", "same");

    let changes = changes_since(&repo.root, &baseline).unwrap();

    assert!(changes.renames.is_empty());
    assert!(changes.deleted.contains("old-a.rs"));
    assert!(changes.deleted.contains("old-b.rs"));
    assert!(changes.added.contains("new.rs"));
}

#[test]
fn git_text_os_propagates_native_git_failure() {
    let repo = Repo::new("git-text-os-error");
    let args = vec![
        std::ffi::OsString::from("rev-parse"),
        std::ffi::OsString::from("--verify"),
        std::ffi::OsString::from("definitely-missing"),
    ];

    assert!(super::git_text_os(&repo.root, args).is_err());
}
