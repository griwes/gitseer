use super::*;

#[test]
fn command_shape_git_bisect_start_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.write("tracked.txt", "second\n");
    repo.git(["commit", "-am", "second"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let bisect_log_path = baseline.snapshot.identity.git_dir.join("BISECT_LOG");

    repo.git(["bisect", "start"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(bisect_log_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(!delta.delta.head_changed);
    assert!(delta.delta.operation_changed);
    assert!(!delta.delta.paths.has_changes());
    let operation = delta.patch.operation.as_ref().unwrap();
    assert_eq!(operation.kind, crate::OperationKind::Bisect);
    assert!(operation.bisect.as_ref().unwrap().good_oids.is_empty());
    assert!(operation.bisect.as_ref().unwrap().bad_oids.is_empty());

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_bisect_bad_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.write("tracked.txt", "second\n");
    repo.git(["commit", "-am", "second"]);
    let bad_oid = repo.git_stdout(["rev-parse", "HEAD"]);
    repo.git(["bisect", "start"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let bisect_log_path = baseline.snapshot.identity.git_dir.join("BISECT_LOG");
    assert!(
        baseline
            .snapshot
            .operation
            .bisect
            .as_ref()
            .unwrap()
            .bad_oids
            .is_empty()
    );

    repo.git(["bisect", "bad"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(bisect_log_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.operation_changed);
    let bisect = delta
        .patch
        .operation
        .as_ref()
        .unwrap()
        .bisect
        .as_ref()
        .unwrap();
    assert_eq!(bisect.bad_oids, vec![bad_oid]);
    assert!(bisect.good_oids.is_empty());

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_bisect_good_commit_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "good\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "good"]);
    let good_oid = repo.git_stdout(["rev-parse", "HEAD"]);
    repo.write("tracked.txt", "middle\n");
    repo.git(["commit", "-am", "middle"]);
    let middle_oid = repo.git_stdout(["rev-parse", "HEAD"]);
    repo.write("tracked.txt", "bad\n");
    repo.git(["commit", "-am", "bad"]);
    let bad_oid = repo.git_stdout(["rev-parse", "HEAD"]);
    repo.git(["bisect", "start"]);
    repo.git(["bisect", "bad"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let bisect_log_path = baseline.snapshot.identity.git_dir.join("BISECT_LOG");

    repo.git(["bisect", "good", good_oid.as_str()]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(bisect_log_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.head_changed);
    assert!(delta.delta.operation_changed);
    assert!(delta.delta.branches_changed);
    assert!(!delta.delta.paths.has_changes());
    assert_eq!(
        delta.patch.head.as_ref().unwrap().oid.as_deref(),
        Some(middle_oid.as_str())
    );
    let bisect = delta
        .patch
        .operation
        .as_ref()
        .unwrap()
        .bisect
        .as_ref()
        .unwrap();
    assert_eq!(bisect.good_oids, vec![good_oid]);
    assert_eq!(bisect.bad_oids, vec![bad_oid]);

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_bisect_reset_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "good\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "good"]);
    let good_oid = repo.git_stdout(["rev-parse", "HEAD"]);
    repo.write("tracked.txt", "middle\n");
    repo.git(["commit", "-am", "middle"]);
    let middle_oid = repo.git_stdout(["rev-parse", "HEAD"]);
    repo.write("tracked.txt", "bad\n");
    repo.git(["commit", "-am", "bad"]);
    let bad_oid = repo.git_stdout(["rev-parse", "HEAD"]);
    repo.git(["bisect", "start"]);
    repo.git(["bisect", "bad"]);
    repo.git(["bisect", "good", good_oid.as_str()]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let bisect_log_path = baseline.snapshot.identity.git_dir.join("BISECT_LOG");
    assert_eq!(
        baseline.snapshot.head.oid.as_deref(),
        Some(middle_oid.as_str())
    );
    assert_eq!(
        baseline.snapshot.operation.kind,
        crate::OperationKind::Bisect
    );

    repo.git(["bisect", "reset"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(bisect_log_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.head_changed);
    assert!(delta.delta.operation_changed);
    assert!(delta.delta.branches_changed);
    assert!(!delta.delta.paths.has_changes());
    assert_eq!(
        delta.patch.operation.as_ref().unwrap().kind,
        crate::OperationKind::Clean
    );
    assert_eq!(
        delta.patch.head.as_ref().unwrap().oid.as_deref(),
        Some(bad_oid.as_str())
    );
    assert_eq!(
        delta.patch.head.as_ref().unwrap().branch.as_deref(),
        Some("main")
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
    assert_eq!(head.oid.as_deref(), Some(bad_oid.as_str()));

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_bisect_skip_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "one\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "one"]);
    let good_oid = repo.git_stdout(["rev-parse", "HEAD"]);
    repo.write("tracked.txt", "two\n");
    repo.git(["commit", "-am", "two"]);
    repo.write("tracked.txt", "three\n");
    repo.git(["commit", "-am", "three"]);
    repo.write("tracked.txt", "four\n");
    repo.git(["commit", "-am", "four"]);
    let bad_oid = repo.git_stdout(["rev-parse", "HEAD"]);
    repo.git(["bisect", "start"]);
    repo.git(["bisect", "bad", bad_oid.as_str()]);
    repo.git(["bisect", "good", good_oid.as_str()]);
    let skipped_oid = repo.git_stdout(["rev-parse", "HEAD"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let bisect_log_path = baseline.snapshot.identity.git_dir.join("BISECT_LOG");
    assert_eq!(
        baseline.snapshot.operation.kind,
        crate::OperationKind::Bisect
    );

    repo.git(["bisect", "skip"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(bisect_log_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.head_changed);
    assert!(delta.delta.operation_changed);
    let bisect = delta
        .patch
        .operation
        .as_ref()
        .unwrap()
        .bisect
        .as_ref()
        .unwrap();
    assert!(bisect.skipped_oids.contains(&skipped_oid));

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}
