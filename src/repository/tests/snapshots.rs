use super::*;

#[test]
fn rejects_non_repository_paths() {
    let temp = TempDir::new().unwrap();

    let error = snapshot_repository(temp.path()).unwrap_err();

    assert!(matches!(error, SnapshotError::NotRepository { .. }));
}

#[test]
fn snapshots_empty_repository_identity() {
    let repo = TestRepo::new();

    let snapshot = snapshot_repository(repo.path()).unwrap();

    assert!(snapshot.identity.is_empty);
    assert!(!snapshot.identity.is_bare);
    assert!(!snapshot.identity.is_shallow);
    assert_eq!(snapshot.identity.namespace, None);
    assert_eq!(snapshot.head.kind, HeadKind::Unborn);
    assert_eq!(snapshot.head.name, None);
    assert_eq!(snapshot.head.branch, None);
}

#[test]
fn snapshots_clean_repository_identity_and_head() {
    let repo = TestRepo::new();
    repo.write("README.md", "hello\n");
    repo.git(["add", "README.md"]);
    repo.git(["commit", "-m", "initial"]);
    repo.git(["tag", "v0.1.0"]);
    repo.git(["tag", "-a", "v0.2.0", "-m", "release v0.2.0"]);

    let snapshot = snapshot_repository(repo.path()).unwrap();

    assert_eq!(
        snapshot.identity.worktree_root.as_deref(),
        Some(repo.path())
    );
    assert!(!snapshot.identity.is_empty);
    assert!(!snapshot.identity.is_shallow);
    assert_eq!(snapshot.identity.namespace, None);
    assert_eq!(snapshot.head.kind, HeadKind::Attached);
    assert_eq!(snapshot.head.name.as_deref(), Some("refs/heads/main"));
    assert!(snapshot.head.branch.is_some());
    assert!(snapshot.head.oid.is_some());
    assert_eq!(
        snapshot
            .head_commit
            .as_ref()
            .and_then(|commit| commit.summary.as_deref()),
        Some("initial")
    );
    assert_eq!(
        snapshot
            .head_commit
            .as_ref()
            .and_then(|commit| commit.author_email.as_deref()),
        Some("tester@example.com")
    );
    assert_eq!(snapshot.operation.kind, OperationKind::Clean);
    assert_eq!(snapshot.operation.message, None);
    assert!(snapshot.operation.heads.is_empty());
    assert!(snapshot.paths.staged.is_empty());
    assert!(snapshot.paths.unstaged.is_empty());
    assert!(snapshot.paths.untracked.is_empty());
    let head_branch = snapshot
        .branches
        .iter()
        .find(|branch| branch.kind == BranchKind::Local && branch.is_head)
        .unwrap();
    assert_eq!(
        head_branch
            .tip_commit
            .as_ref()
            .and_then(|commit| commit.summary.as_deref()),
        Some("initial")
    );
    let lightweight_tag = snapshot
        .tags
        .iter()
        .find(|tag| tag.name == "v0.1.0")
        .unwrap();
    assert_eq!(lightweight_tag.kind, TagKind::Lightweight);
    assert_eq!(lightweight_tag.oid, lightweight_tag.target_oid);
    assert_eq!(lightweight_tag.target_kind, Some(GitObjectKind::Commit));
    assert_eq!(lightweight_tag.message, None);
    let annotated_tag = snapshot
        .tags
        .iter()
        .find(|tag| tag.name == "v0.2.0")
        .unwrap();
    assert_eq!(annotated_tag.kind, TagKind::Annotated);
    assert_ne!(annotated_tag.oid, annotated_tag.target_oid);
    assert_eq!(annotated_tag.target_kind, Some(GitObjectKind::Commit));
    assert_eq!(
        annotated_tag.tagger_email.as_deref(),
        Some("tester@example.com")
    );
    assert_eq!(annotated_tag.message.as_deref(), Some("release v0.2.0\n"));
}

#[test]
fn snapshots_detached_head_oid() {
    let repo = TestRepo::new();
    repo.write("README.md", "hello\n");
    repo.git(["add", "README.md"]);
    repo.git(["commit", "-m", "initial"]);
    let head_oid = repo.git_stdout(["rev-parse", "HEAD"]);
    repo.git(["checkout", "--detach", &head_oid]);

    let snapshot = snapshot_repository(repo.path()).unwrap();

    assert_eq!(snapshot.head.kind, HeadKind::Detached);
    assert_eq!(snapshot.head.branch, None);
    assert_eq!(snapshot.head.oid.as_deref(), Some(head_oid.as_str()));
    assert_eq!(
        snapshot.head_commit.and_then(|commit| commit.summary),
        Some("initial".to_string())
    );
}

#[test]
fn snapshots_round_trip_through_json() {
    let repo = TestRepo::new();
    repo.write("README.md", "hello\n");
    repo.git(["add", "README.md"]);
    repo.git(["commit", "-m", "initial"]);
    let snapshot = snapshot_repository(repo.path()).unwrap();

    let serialized = serde_json::to_string(&snapshot).unwrap();
    let deserialized: RepositorySnapshot = serde_json::from_str(&serialized).unwrap();

    assert_eq!(deserialized, snapshot);
}

#[test]
fn deserializes_operation_state_without_heads() {
    let operation: OperationState =
        serde_json::from_str(r#"{"kind":"clean","message":null}"#).unwrap();

    assert_eq!(operation.kind, OperationKind::Clean);
    assert_eq!(operation.message, None);
    assert!(operation.heads.is_empty());
}
