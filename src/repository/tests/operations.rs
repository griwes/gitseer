use super::*;

#[test]
fn detects_merge_operation_and_conflicted_paths() {
    let repo = TestRepo::new();
    repo.write("conflict.txt", "base\n");
    repo.git(["add", "conflict.txt"]);
    repo.git(["commit", "-m", "initial"]);
    let base_branch = repo.current_branch();

    repo.git(["checkout", "-b", "side"]);
    repo.write("conflict.txt", "side\n");
    repo.git(["commit", "-am", "side"]);

    repo.git(["checkout", &base_branch]);
    repo.write("conflict.txt", "main\n");
    repo.git(["commit", "-am", "main"]);
    repo.git_expect_failure(["merge", "side"]);

    let snapshot = snapshot_repository(repo.path()).unwrap();

    assert_eq!(snapshot.operation.kind, OperationKind::Merge);
    assert!(snapshot.operation.message.is_some());
    let merge_head = snapshot
        .operation
        .heads
        .iter()
        .find(|head| head.role == OperationHeadRole::Merge)
        .unwrap();
    assert_eq!(merge_head.oid, repo.git_stdout(["rev-parse", "side"]));
    assert_eq!(snapshot.paths.conflicted, vec!["conflict.txt"]);
    assert_eq!(snapshot.paths.conflicts.len(), 1);
    let conflict = &snapshot.paths.conflicts[0];
    assert_eq!(conflict.path, "conflict.txt");
    let ancestor = conflict.ancestor.as_ref().unwrap();
    let ours = conflict.ours.as_ref().unwrap();
    let theirs = conflict.theirs.as_ref().unwrap();
    assert_eq!(ancestor.path, "conflict.txt");
    assert_eq!(ours.path, "conflict.txt");
    assert_eq!(theirs.path, "conflict.txt");
    assert_eq!(ancestor.mode, 0o100644);
    assert_eq!(ours.mode, 0o100644);
    assert_eq!(theirs.mode, 0o100644);
    assert_ne!(ancestor.oid, ours.oid);
    assert_ne!(ours.oid, theirs.oid);
}

#[test]
fn detects_rebase_operation_and_head_oid() {
    let repo = TestRepo::new();
    repo.write("conflict.txt", "base\n");
    repo.git(["add", "conflict.txt"]);
    repo.git(["commit", "-m", "initial"]);
    let base_branch = repo.current_branch();

    repo.git(["checkout", "-b", "side"]);
    repo.write("conflict.txt", "side\n");
    repo.git(["commit", "-am", "side"]);
    let rebased_oid = repo.git_stdout(["rev-parse", "HEAD"]);

    repo.git(["checkout", &base_branch]);
    repo.write("conflict.txt", "main\n");
    repo.git(["commit", "-am", "main"]);

    repo.git(["checkout", "side"]);
    repo.git_expect_failure(["rebase", &base_branch]);

    let snapshot = snapshot_repository(repo.path()).unwrap();

    assert!(matches!(
        snapshot.operation.kind,
        OperationKind::Rebase | OperationKind::RebaseInteractive | OperationKind::RebaseMerge
    ));
    let rebase_head = snapshot
        .operation
        .heads
        .iter()
        .find(|head| head.role == OperationHeadRole::Rebase)
        .unwrap();
    assert_eq!(rebase_head.oid, rebased_oid);
    assert_eq!(snapshot.paths.conflicted, vec!["conflict.txt"]);
}

#[test]
fn detects_cherry_pick_operation_and_head_oid() {
    let repo = TestRepo::new();
    repo.write("conflict.txt", "base\n");
    repo.git(["add", "conflict.txt"]);
    repo.git(["commit", "-m", "initial"]);
    let base_branch = repo.current_branch();

    repo.git(["checkout", "-b", "side"]);
    repo.write("conflict.txt", "side\n");
    repo.git(["commit", "-am", "side"]);
    let picked_oid = repo.git_stdout(["rev-parse", "HEAD"]);

    repo.git(["checkout", &base_branch]);
    repo.write("conflict.txt", "main\n");
    repo.git(["commit", "-am", "main"]);
    repo.git_expect_failure(["cherry-pick", &picked_oid]);

    let snapshot = snapshot_repository(repo.path()).unwrap();

    assert!(matches!(
        snapshot.operation.kind,
        OperationKind::CherryPick | OperationKind::CherryPickSequence
    ));
    let cherry_pick_head = snapshot
        .operation
        .heads
        .iter()
        .find(|head| head.role == OperationHeadRole::CherryPick)
        .unwrap();
    assert_eq!(cherry_pick_head.oid, picked_oid);
    assert_eq!(snapshot.paths.conflicted, vec!["conflict.txt"]);
}

#[test]
fn detects_revert_operation_and_head_oid() {
    let repo = TestRepo::new();
    repo.write("conflict.txt", "base\n");
    repo.git(["add", "conflict.txt"]);
    repo.git(["commit", "-m", "initial"]);

    repo.write("conflict.txt", "target\n");
    repo.git(["commit", "-am", "target"]);
    let reverted_oid = repo.git_stdout(["rev-parse", "HEAD"]);

    repo.write("conflict.txt", "current\n");
    repo.git(["commit", "-am", "current"]);
    repo.git_expect_failure(["revert", &reverted_oid]);

    let snapshot = snapshot_repository(repo.path()).unwrap();

    assert!(matches!(
        snapshot.operation.kind,
        OperationKind::Revert | OperationKind::RevertSequence
    ));
    let revert_head = snapshot
        .operation
        .heads
        .iter()
        .find(|head| head.role == OperationHeadRole::Revert)
        .unwrap();
    assert_eq!(revert_head.oid, reverted_oid);
    assert_eq!(snapshot.paths.conflicted, vec!["conflict.txt"]);
}

#[test]
fn detects_bisect_operation_and_refs() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "good\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "good"]);
    let good_oid = repo.git_stdout(["rev-parse", "HEAD"]);

    repo.write("tracked.txt", "middle\n");
    repo.git(["commit", "-am", "middle"]);

    repo.write("tracked.txt", "bad\n");
    repo.git(["commit", "-am", "bad"]);
    let bad_oid = repo.git_stdout(["rev-parse", "HEAD"]);

    repo.git(["bisect", "start", &bad_oid, &good_oid]);

    let snapshot = snapshot_repository(repo.path()).unwrap();

    assert_eq!(snapshot.operation.kind, OperationKind::Bisect);
    assert!(snapshot.operation.heads.is_empty());
    let bisect = snapshot.operation.bisect.as_ref().unwrap();
    assert_eq!(bisect.good_oids, vec![good_oid]);
    assert_eq!(bisect.bad_oids, vec![bad_oid]);
    assert!(bisect.skipped_oids.is_empty());
}
