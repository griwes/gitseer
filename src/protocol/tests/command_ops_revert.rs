use super::*;

#[test]
fn command_shape_clean_git_revert_commit_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.write("tracked.txt", "change\n");
    repo.git(["commit", "-am", "change"]);
    let reverted_oid = repo.git_stdout(["rev-parse", "HEAD"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let main_ref_path = baseline.snapshot.identity.git_dir.join("refs/heads/main");

    repo.git(["revert", reverted_oid.as_str()]);
    let revert_oid = repo.git_stdout(["rev-parse", "main"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(main_ref_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.head_changed);
    assert!(delta.delta.branches_changed);
    assert!(!delta.delta.paths.has_changes());
    assert_ne!(revert_oid, reverted_oid);
    assert_eq!(
        delta.patch.head.as_ref().unwrap().oid.as_deref(),
        Some(revert_oid.as_str())
    );
    let head = delta
        .patch
        .branches
        .as_ref()
        .unwrap()
        .iter()
        .find(|branch| branch.is_head)
        .unwrap();
    assert_eq!(head.name, "main");
    assert_eq!(head.oid.as_deref(), Some(revert_oid.as_str()));

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_conflicted_git_revert_commit_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("conflict.txt", "base\n");
    repo.git(["add", "conflict.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.write("conflict.txt", "reverted\n");
    repo.git(["commit", "-am", "to revert"]);
    let reverted_oid = repo.git_stdout(["rev-parse", "HEAD"]);
    repo.write("conflict.txt", "current\n");
    repo.git(["commit", "-am", "current"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let revert_head_path = baseline.snapshot.identity.git_dir.join("REVERT_HEAD");

    repo.git_expect_failure(["revert", reverted_oid.as_str()]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(revert_head_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(!delta.delta.head_changed);
    assert!(delta.delta.operation_changed);
    assert!(delta.delta.paths.has_changes());
    let operation = delta.patch.operation.as_ref().unwrap();
    assert!(matches!(
        operation.kind,
        crate::OperationKind::Revert | crate::OperationKind::RevertSequence
    ));
    let revert_head = operation
        .heads
        .iter()
        .find(|head| head.role == crate::OperationHeadRole::Revert)
        .unwrap();
    assert_eq!(revert_head.oid, reverted_oid);
    let paths = delta.patch.paths.as_ref().unwrap();
    assert_eq!(paths.conflicted, vec!["conflict.txt"]);
    assert_eq!(paths.conflicts.len(), 1);

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_revert_abort_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("conflict.txt", "base\n");
    repo.git(["add", "conflict.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.write("conflict.txt", "reverted\n");
    repo.git(["commit", "-am", "to revert"]);
    let reverted_oid = repo.git_stdout(["rev-parse", "HEAD"]);
    repo.write("conflict.txt", "current\n");
    repo.git(["commit", "-am", "current"]);
    repo.git_expect_failure(["revert", reverted_oid.as_str()]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let revert_head_path = baseline.snapshot.identity.git_dir.join("REVERT_HEAD");
    assert!(matches!(
        baseline.snapshot.operation.kind,
        crate::OperationKind::Revert | crate::OperationKind::RevertSequence
    ));
    assert_eq!(baseline.snapshot.paths.conflicted, vec!["conflict.txt"]);

    repo.git(["revert", "--abort"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(revert_head_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(!delta.delta.head_changed);
    assert!(delta.delta.operation_changed);
    assert!(delta.delta.paths.has_changes());
    assert_eq!(
        delta.patch.operation.as_ref().unwrap().kind,
        crate::OperationKind::Clean
    );
    let paths = delta.patch.paths.as_ref().unwrap();
    assert!(paths.conflicted.is_empty());
    assert!(paths.conflicts.is_empty());
    assert!(paths.unstaged.is_empty());

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_revert_continue_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("conflict.txt", "base\n");
    repo.git(["add", "conflict.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.write("conflict.txt", "reverted\n");
    repo.git(["commit", "-am", "to revert"]);
    let reverted_oid = repo.git_stdout(["rev-parse", "HEAD"]);
    repo.write("conflict.txt", "current\n");
    repo.git(["commit", "-am", "current"]);
    let main_before_continue = repo.git_stdout(["rev-parse", "main"]);
    repo.git_expect_failure(["revert", reverted_oid.as_str()]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let revert_head_path = baseline.snapshot.identity.git_dir.join("REVERT_HEAD");
    assert!(matches!(
        baseline.snapshot.operation.kind,
        crate::OperationKind::Revert | crate::OperationKind::RevertSequence
    ));
    assert_eq!(baseline.snapshot.paths.conflicted, vec!["conflict.txt"]);

    repo.write("conflict.txt", "resolved\n");
    repo.git(["add", "conflict.txt"]);
    repo.git(["revert", "--continue"]);
    let continued_oid = repo.git_stdout(["rev-parse", "main"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(revert_head_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.head_changed);
    assert!(delta.delta.operation_changed);
    assert!(delta.delta.branches_changed);
    assert!(delta.delta.paths.has_changes());
    assert_ne!(continued_oid, main_before_continue);
    assert_eq!(
        delta.patch.operation.as_ref().unwrap().kind,
        crate::OperationKind::Clean
    );
    assert_eq!(
        delta.patch.head.as_ref().unwrap().oid.as_deref(),
        Some(continued_oid.as_str())
    );
    let head = delta
        .patch
        .branches
        .as_ref()
        .unwrap()
        .iter()
        .find(|branch| branch.is_head)
        .unwrap();
    assert_eq!(head.name, "main");
    assert_eq!(head.oid.as_deref(), Some(continued_oid.as_str()));
    let paths = delta.patch.paths.as_ref().unwrap();
    assert!(paths.conflicted.is_empty());
    assert!(paths.staged.is_empty());
    assert!(paths.unstaged.is_empty());

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_revert_skip_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("conflict.txt", "base\n");
    repo.git(["add", "conflict.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.write("conflict.txt", "reverted\n");
    repo.git(["commit", "-am", "to revert"]);
    let reverted_oid = repo.git_stdout(["rev-parse", "HEAD"]);
    repo.write("conflict.txt", "current\n");
    repo.git(["commit", "-am", "current"]);
    let main_before_skip = repo.git_stdout(["rev-parse", "main"]);
    repo.git_expect_failure(["revert", reverted_oid.as_str()]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let revert_head_path = baseline.snapshot.identity.git_dir.join("REVERT_HEAD");
    assert!(matches!(
        baseline.snapshot.operation.kind,
        crate::OperationKind::Revert | crate::OperationKind::RevertSequence
    ));
    assert_eq!(baseline.snapshot.paths.conflicted, vec!["conflict.txt"]);

    repo.git(["revert", "--skip"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(revert_head_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(!delta.delta.head_changed);
    assert!(delta.delta.operation_changed);
    assert!(delta.delta.paths.has_changes());
    assert!(delta.patch.head.is_none());
    assert_eq!(
        snapshot_repository(repo.path())
            .unwrap()
            .head
            .oid
            .as_deref(),
        Some(main_before_skip.as_str())
    );
    assert_eq!(
        delta.patch.operation.as_ref().unwrap().kind,
        crate::OperationKind::Clean
    );
    assert!(delta.patch.paths.as_ref().unwrap().conflicted.is_empty());

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}
