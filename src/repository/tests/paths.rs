use super::*;

#[test]
fn separates_staged_unstaged_and_untracked_paths() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);

    repo.write("tracked.txt", "changed\n");
    repo.write("staged.txt", "staged\n");
    repo.git(["add", "staged.txt"]);
    repo.write("untracked.txt", "untracked\n");

    let snapshot = snapshot_repository(repo.path()).unwrap();

    assert_eq!(snapshot.paths.staged, vec!["staged.txt"]);
    assert_eq!(snapshot.paths.unstaged, vec!["tracked.txt"]);
    assert_eq!(snapshot.paths.untracked, vec!["untracked.txt"]);
    assert!(snapshot.paths.conflicted.is_empty());
    let staged = snapshot
        .paths
        .entries
        .iter()
        .find(|entry| entry.path == "staged.txt")
        .unwrap();
    assert!(staged.status.index_new);
    assert_eq!(staged.staged_new_path.as_deref(), Some("staged.txt"));
    let unstaged = snapshot
        .paths
        .entries
        .iter()
        .find(|entry| entry.path == "tracked.txt")
        .unwrap();
    assert!(unstaged.status.workdir_modified);
    assert_eq!(unstaged.workdir_new_path.as_deref(), Some("tracked.txt"));
    let untracked = snapshot
        .paths
        .entries
        .iter()
        .find(|entry| entry.path == "untracked.txt")
        .unwrap();
    assert!(untracked.status.workdir_new);
}

#[test]
fn omits_ignored_paths_by_default_and_includes_when_requested() {
    let repo = TestRepo::new();
    repo.write(".gitignore", "ignored.txt\n");
    repo.git(["add", ".gitignore"]);
    repo.git(["commit", "-m", "ignore rules"]);
    repo.write("ignored.txt", "ignored\n");

    let default_snapshot = snapshot_repository(repo.path()).unwrap();

    assert!(default_snapshot.paths.ignored.is_empty());
    assert!(
        default_snapshot
            .paths
            .entries
            .iter()
            .all(|entry| entry.path != "ignored.txt")
    );

    let snapshot = snapshot_repository_with_options(
        repo.path(),
        SnapshotOptions {
            include_ignored: true,
        },
    )
    .unwrap();

    assert_eq!(snapshot.paths.ignored, vec!["ignored.txt"]);
    let ignored = snapshot
        .paths
        .entries
        .iter()
        .find(|entry| entry.path == "ignored.txt")
        .unwrap();
    assert!(ignored.status.ignored);
}

#[test]
fn deserializes_path_state_without_ignored_paths() {
    let path_state: PathState = serde_json::from_str(
        r#"{"staged":[],"unstaged":[],"untracked":[],"conflicted":[],"conflicts":[],"entries":[]}"#,
    )
    .unwrap();

    assert!(path_state.ignored.is_empty());
}

#[test]
fn computes_coarse_snapshot_deltas() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    let previous = snapshot_repository(repo.path()).unwrap();

    repo.write("tracked.txt", "changed\n");
    repo.write("new.txt", "new\n");
    let current = snapshot_repository(repo.path()).unwrap();

    let delta = snapshot_delta(&previous, &current);

    assert!(!delta.head_changed);
    assert!(!delta.operation_changed);
    assert_eq!(delta.paths.unstaged.added, vec!["tracked.txt"]);
    assert_eq!(delta.paths.untracked.added, vec!["new.txt"]);
    assert!(
        delta
            .paths
            .entries_changed
            .contains(&"tracked.txt".to_string())
    );
    assert!(delta.paths.entries_changed.contains(&"new.txt".to_string()));
}

#[test]
fn delta_reports_identity_changes() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    let previous = snapshot_repository(repo.path()).unwrap();
    let mut current = previous.clone();
    current.identity.namespace = Some("namespace".to_string());

    let delta = snapshot_delta(&previous, &current);
    let patch = SnapshotPatch::from_delta(&current, &delta);

    assert!(delta.identity_changed);
    assert_eq!(
        patch.identity.and_then(|identity| identity.namespace),
        Some("namespace".to_string())
    );
}

#[test]
fn includes_commit_parent_oids() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    let first_commit = repo.git_stdout(["rev-parse", "HEAD"]);
    repo.write("tracked.txt", "second\n");
    repo.git(["commit", "-am", "second"]);

    let snapshot = snapshot_repository(repo.path()).unwrap();

    let head_commit = snapshot.head_commit.as_ref().unwrap();
    assert_eq!(head_commit.summary.as_deref(), Some("second"));
    assert_eq!(head_commit.parent_oids, vec![first_commit]);
    let head_branch = snapshot
        .branches
        .iter()
        .find(|branch| branch.is_head)
        .unwrap();
    assert_eq!(
        head_branch
            .tip_commit
            .as_ref()
            .map(|commit| &commit.parent_oids),
        Some(&head_commit.parent_oids)
    );
}
