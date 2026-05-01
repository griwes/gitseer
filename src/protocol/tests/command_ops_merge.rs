use super::*;

#[test]
fn command_shape_clean_git_merge_branch_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.git(["checkout", "-b", "side"]);
    repo.write("tracked.txt", "side\n");
    repo.git(["commit", "-am", "side"]);
    let side_oid = repo.git_stdout(["rev-parse", "side"]);
    repo.git(["checkout", "main"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let main_ref_path = baseline.snapshot.identity.git_dir.join("refs/heads/main");
    assert_ne!(
        baseline.snapshot.head.oid.as_deref(),
        Some(side_oid.as_str())
    );

    repo.git(["merge", "side"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(main_ref_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.head_changed);
    assert!(delta.delta.branches_changed);
    assert!(!delta.delta.paths.has_changes());
    assert_eq!(
        delta.patch.head.as_ref().unwrap().oid.as_deref(),
        Some(side_oid.as_str())
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
    assert_eq!(head.oid.as_deref(), Some(side_oid.as_str()));

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_conflicted_git_merge_branch_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("conflict.txt", "base\n");
    repo.git(["add", "conflict.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.git(["checkout", "-b", "side"]);
    repo.write("conflict.txt", "side\n");
    repo.git(["commit", "-am", "side"]);
    let side_oid = repo.git_stdout(["rev-parse", "side"]);
    repo.git(["checkout", "main"]);
    repo.write("conflict.txt", "main\n");
    repo.git(["commit", "-am", "main"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let merge_head_path = baseline.snapshot.identity.git_dir.join("MERGE_HEAD");

    repo.git_expect_failure(["merge", "side"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(merge_head_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(!delta.delta.head_changed);
    assert!(delta.delta.operation_changed);
    assert!(delta.delta.paths.has_changes());
    let operation = delta.patch.operation.as_ref().unwrap();
    assert_eq!(operation.kind, crate::OperationKind::Merge);
    let merge_head = operation
        .heads
        .iter()
        .find(|head| head.role == crate::OperationHeadRole::Merge)
        .unwrap();
    assert_eq!(merge_head.oid, side_oid);
    let paths = delta.patch.paths.as_ref().unwrap();
    assert_eq!(paths.conflicted, vec!["conflict.txt"]);
    assert_eq!(paths.conflicts.len(), 1);

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_merge_abort_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("conflict.txt", "base\n");
    repo.git(["add", "conflict.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.git(["checkout", "-b", "side"]);
    repo.write("conflict.txt", "side\n");
    repo.git(["commit", "-am", "side"]);
    repo.git(["checkout", "main"]);
    repo.write("conflict.txt", "main\n");
    repo.git(["commit", "-am", "main"]);
    repo.git_expect_failure(["merge", "side"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let merge_head_path = baseline.snapshot.identity.git_dir.join("MERGE_HEAD");
    assert_eq!(
        baseline.snapshot.operation.kind,
        crate::OperationKind::Merge
    );
    assert_eq!(baseline.snapshot.paths.conflicted, vec!["conflict.txt"]);

    repo.git(["merge", "--abort"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(merge_head_path));

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
fn command_shape_git_merge_continue_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("conflict.txt", "base\n");
    repo.git(["add", "conflict.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.git(["checkout", "-b", "side"]);
    repo.write("conflict.txt", "side\n");
    repo.git(["commit", "-am", "side"]);
    repo.git(["checkout", "main"]);
    repo.write("conflict.txt", "main\n");
    repo.git(["commit", "-am", "main"]);
    let main_before_continue = repo.git_stdout(["rev-parse", "main"]);
    repo.git_expect_failure(["merge", "side"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let merge_head_path = baseline.snapshot.identity.git_dir.join("MERGE_HEAD");
    assert_eq!(
        baseline.snapshot.operation.kind,
        crate::OperationKind::Merge
    );
    assert_eq!(baseline.snapshot.paths.conflicted, vec!["conflict.txt"]);

    repo.write("conflict.txt", "resolved\n");
    repo.git(["add", "conflict.txt"]);
    repo.git(["merge", "--continue"]);
    let merge_oid = repo.git_stdout(["rev-parse", "main"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(merge_head_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.head_changed);
    assert!(delta.delta.operation_changed);
    assert!(delta.delta.branches_changed);
    assert!(delta.delta.paths.has_changes());
    assert_ne!(merge_oid, main_before_continue);
    assert_eq!(
        delta.patch.operation.as_ref().unwrap().kind,
        crate::OperationKind::Clean
    );
    assert_eq!(
        delta.patch.head.as_ref().unwrap().oid.as_deref(),
        Some(merge_oid.as_str())
    );
    let head = delta
        .patch
        .branches
        .as_ref()
        .unwrap()
        .iter()
        .find(|branch| branch.is_head)
        .unwrap();
    assert_eq!(head.oid.as_deref(), Some(merge_oid.as_str()));
    let paths = delta.patch.paths.as_ref().unwrap();
    assert!(paths.conflicted.is_empty());
    assert!(paths.staged.is_empty());
    assert!(paths.unstaged.is_empty());

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}
