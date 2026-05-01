use super::*;

#[test]
fn command_shape_clean_git_rebase_branch_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("base.txt", "base\n");
    repo.git(["add", "base.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.git(["checkout", "-b", "side"]);
    repo.write("side.txt", "side\n");
    repo.git(["add", "side.txt"]);
    repo.git(["commit", "-m", "side"]);
    let side_before_rebase = repo.git_stdout(["rev-parse", "side"]);
    repo.git(["checkout", "main"]);
    repo.write("main.txt", "main\n");
    repo.git(["add", "main.txt"]);
    repo.git(["commit", "-m", "main"]);
    repo.git(["checkout", "side"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let side_ref_path = baseline.snapshot.identity.git_dir.join("refs/heads/side");
    assert_eq!(
        baseline.snapshot.head.oid.as_deref(),
        Some(side_before_rebase.as_str())
    );

    repo.git(["rebase", "main"]);
    let rebased_oid = repo.git_stdout(["rev-parse", "side"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(side_ref_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.head_changed);
    assert!(delta.delta.branches_changed);
    assert!(!delta.delta.paths.has_changes());
    assert_ne!(rebased_oid, side_before_rebase);
    assert_eq!(
        delta.patch.head.as_ref().unwrap().oid.as_deref(),
        Some(rebased_oid.as_str())
    );
    let head = delta
        .patch
        .branches
        .as_ref()
        .unwrap()
        .iter()
        .find(|branch| branch.is_head)
        .unwrap();
    assert_eq!(head.name, "side");
    assert_eq!(head.oid.as_deref(), Some(rebased_oid.as_str()));

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_conflicted_git_rebase_branch_emits_patchable_delta() {
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
    repo.git(["checkout", "side"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let rebase_head_path = baseline.snapshot.identity.git_dir.join("REBASE_HEAD");
    assert_eq!(
        baseline.snapshot.head.oid.as_deref(),
        Some(side_oid.as_str())
    );

    repo.git_expect_failure(["rebase", "main"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(rebase_head_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.head_changed);
    assert!(delta.delta.operation_changed);
    assert!(delta.delta.paths.has_changes());
    let operation = delta.patch.operation.as_ref().unwrap();
    assert!(matches!(
        operation.kind,
        crate::OperationKind::Rebase
            | crate::OperationKind::RebaseInteractive
            | crate::OperationKind::RebaseMerge
    ));
    let rebase_head = operation
        .heads
        .iter()
        .find(|head| head.role == crate::OperationHeadRole::Rebase)
        .unwrap();
    assert_eq!(rebase_head.oid, side_oid);
    let paths = delta.patch.paths.as_ref().unwrap();
    assert_eq!(paths.conflicted, vec!["conflict.txt"]);
    assert_eq!(paths.conflicts.len(), 1);

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_rebase_abort_emits_patchable_delta() {
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
    repo.git(["checkout", "side"]);
    repo.git_expect_failure(["rebase", "main"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let rebase_head_path = baseline.snapshot.identity.git_dir.join("REBASE_HEAD");
    assert!(matches!(
        baseline.snapshot.operation.kind,
        crate::OperationKind::Rebase
            | crate::OperationKind::RebaseInteractive
            | crate::OperationKind::RebaseMerge
    ));
    assert_eq!(baseline.snapshot.paths.conflicted, vec!["conflict.txt"]);

    repo.git(["rebase", "--abort"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(rebase_head_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.head_changed);
    assert!(delta.delta.operation_changed);
    assert!(delta.delta.branches_changed);
    assert!(delta.delta.paths.has_changes());
    assert_eq!(
        delta.patch.operation.as_ref().unwrap().kind,
        crate::OperationKind::Clean
    );
    assert_eq!(
        delta.patch.head.as_ref().unwrap().oid.as_deref(),
        Some(side_oid.as_str())
    );
    assert_eq!(
        delta.patch.head.as_ref().unwrap().branch.as_deref(),
        Some("side")
    );
    let paths = delta.patch.paths.as_ref().unwrap();
    assert!(paths.conflicted.is_empty());
    assert!(paths.conflicts.is_empty());

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_rebase_continue_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("conflict.txt", "base\n");
    repo.git(["add", "conflict.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.git(["checkout", "-b", "side"]);
    repo.write("conflict.txt", "side\n");
    repo.git(["commit", "-am", "side"]);
    let side_before_rebase = repo.git_stdout(["rev-parse", "side"]);
    repo.git(["checkout", "main"]);
    repo.write("conflict.txt", "main\n");
    repo.git(["commit", "-am", "main"]);
    repo.git(["checkout", "side"]);
    repo.git_expect_failure(["rebase", "main"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let rebase_head_path = baseline.snapshot.identity.git_dir.join("REBASE_HEAD");
    assert!(matches!(
        baseline.snapshot.operation.kind,
        crate::OperationKind::Rebase
            | crate::OperationKind::RebaseInteractive
            | crate::OperationKind::RebaseMerge
    ));
    assert_eq!(baseline.snapshot.paths.conflicted, vec!["conflict.txt"]);

    repo.write("conflict.txt", "resolved\n");
    repo.git(["add", "conflict.txt"]);
    repo.git(["rebase", "--continue"]);
    let rebased_oid = repo.git_stdout(["rev-parse", "side"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(rebase_head_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.head_changed);
    assert!(delta.delta.operation_changed);
    assert!(delta.delta.branches_changed);
    assert!(delta.delta.paths.has_changes());
    assert_ne!(rebased_oid, side_before_rebase);
    assert_eq!(
        delta.patch.operation.as_ref().unwrap().kind,
        crate::OperationKind::Clean
    );
    assert_eq!(
        delta.patch.head.as_ref().unwrap().oid.as_deref(),
        Some(rebased_oid.as_str())
    );
    assert_eq!(
        delta.patch.head.as_ref().unwrap().branch.as_deref(),
        Some("side")
    );
    let head = delta
        .patch
        .branches
        .as_ref()
        .unwrap()
        .iter()
        .find(|branch| branch.is_head)
        .unwrap();
    assert_eq!(head.name, "side");
    assert_eq!(head.oid.as_deref(), Some(rebased_oid.as_str()));
    let paths = delta.patch.paths.as_ref().unwrap();
    assert!(paths.conflicted.is_empty());
    assert!(paths.staged.is_empty());
    assert!(paths.unstaged.is_empty());

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_rebase_skip_emits_patchable_delta() {
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
    let main_oid = repo.git_stdout(["rev-parse", "main"]);
    repo.git(["checkout", "side"]);
    repo.git_expect_failure(["rebase", "main"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let rebase_head_path = baseline.snapshot.identity.git_dir.join("REBASE_HEAD");
    assert!(matches!(
        baseline.snapshot.operation.kind,
        crate::OperationKind::Rebase
            | crate::OperationKind::RebaseInteractive
            | crate::OperationKind::RebaseMerge
    ));
    assert_eq!(baseline.snapshot.paths.conflicted, vec!["conflict.txt"]);

    repo.git(["rebase", "--skip"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(rebase_head_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.head_changed);
    assert!(delta.delta.operation_changed);
    assert!(delta.delta.branches_changed);
    assert!(delta.delta.paths.has_changes());
    assert_eq!(
        delta.patch.operation.as_ref().unwrap().kind,
        crate::OperationKind::Clean
    );
    assert_eq!(
        delta.patch.head.as_ref().unwrap().oid.as_deref(),
        Some(main_oid.as_str())
    );
    assert_eq!(
        delta.patch.head.as_ref().unwrap().branch.as_deref(),
        Some("side")
    );
    assert!(delta.patch.paths.as_ref().unwrap().conflicted.is_empty());

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}
