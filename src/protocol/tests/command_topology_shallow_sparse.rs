use super::*;

#[test]
fn command_shape_git_fetch_deepen_emits_patchable_delta() {
    let remote = TestRepo::new();
    remote.write("tracked.txt", "one\n");
    remote.git(["add", "tracked.txt"]);
    remote.git(["commit", "-m", "one"]);
    remote.write("tracked.txt", "two\n");
    remote.git(["commit", "-am", "two"]);
    remote.write("tracked.txt", "three\n");
    remote.git(["commit", "-am", "three"]);
    let repo = TestRepo::shallow_clone_from(remote.path());
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    assert!(baseline.snapshot.identity.is_shallow);
    let shallow_path = baseline.snapshot.identity.git_dir.join("shallow");

    repo.git_allow_file_protocol(["fetch", "--deepen", "1", "origin"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(shallow_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(!delta.delta.identity_changed);
    assert!(delta.delta.head_changed);
    assert!(delta.delta.branches_changed);
    assert!(!delta.delta.paths.has_changes());
    assert_eq!(delta.patch.identity, None);
    assert_eq!(delta.patch.paths, None);
    assert!(
        !delta
            .patch
            .head_commit
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap()
            .parent_oids
            .is_empty()
    );

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_fetch_unshallow_emits_patchable_delta() {
    let remote = TestRepo::new();
    remote.write("tracked.txt", "one\n");
    remote.git(["add", "tracked.txt"]);
    remote.git(["commit", "-m", "one"]);
    remote.write("tracked.txt", "two\n");
    remote.git(["commit", "-am", "two"]);
    let repo = TestRepo::shallow_clone_from(remote.path());
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    assert!(baseline.snapshot.identity.is_shallow);
    let shallow_path = baseline.snapshot.identity.git_dir.join("shallow");

    repo.git_allow_file_protocol(["fetch", "--unshallow", "origin"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(shallow_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.identity_changed);
    assert!(delta.delta.head_changed);
    assert!(delta.delta.branches_changed);
    assert!(!delta.delta.paths.has_changes());
    assert!(!delta.patch.identity.as_ref().unwrap().is_shallow);

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_sparse_checkout_set_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("visible/file.txt", "visible\n");
    repo.write("hidden/file.txt", "hidden\n");
    repo.git(["add", "visible/file.txt", "hidden/file.txt"]);
    repo.git(["commit", "-m", "initial"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let sparse_path = baseline
        .snapshot
        .identity
        .git_dir
        .join("info/sparse-checkout");

    repo.git(["sparse-checkout", "set", "visible"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(sparse_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.paths.has_changes());
    assert!(
        delta
            .delta
            .paths
            .entries_changed
            .contains(&"hidden/file.txt".to_string())
    );
    let hidden = delta
        .patch
        .paths
        .as_ref()
        .unwrap()
        .entries
        .iter()
        .find(|entry| entry.path == "hidden/file.txt")
        .unwrap();
    assert!(hidden.status.skip_worktree);

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_sparse_checkout_disable_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("visible/file.txt", "visible\n");
    repo.write("hidden/file.txt", "hidden\n");
    repo.git(["add", "visible/file.txt", "hidden/file.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.git(["sparse-checkout", "set", "visible"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    assert!(
        baseline
            .snapshot
            .paths
            .entries
            .iter()
            .any(|entry| entry.path == "hidden/file.txt" && entry.status.skip_worktree)
    );
    let sparse_path = baseline
        .snapshot
        .identity
        .git_dir
        .join("info/sparse-checkout");

    repo.git(["sparse-checkout", "disable"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(sparse_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.paths.has_changes());
    let hidden = delta
        .patch
        .paths
        .as_ref()
        .unwrap()
        .entries
        .iter()
        .find(|entry| entry.path == "hidden/file.txt");
    assert!(hidden.is_none_or(|entry| !entry.status.skip_worktree));

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}
