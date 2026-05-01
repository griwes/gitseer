use super::*;

#[test]
fn command_shape_git_init_nested_repo_emits_no_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);

    repo.git(["init", "--initial-branch=main", "nested"]);
    let plan = update_from_watch_event_with_no_delta(
        &mut state,
        event_for(repo.path().join("nested/.git/HEAD")),
    );

    assert_incremental_refresh_plan(&plan);
    let refresh = refresh_repository_with_plan(
        state.repo(),
        Some(&baseline.snapshot),
        &plan,
        SnapshotOptions::default(),
    )
    .unwrap();
    assert_eq!(refresh.plan, plan);
    assert!(snapshot_update_messages(&mut state, refresh.snapshot).is_empty());
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(fresh, baseline.snapshot);
}

#[test]
fn command_shape_git_init_bare_external_repo_emits_no_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);

    let remote_parent = TempDir::new().unwrap();
    let remote_path = remote_parent.path().join("remote.git");
    init_bare_repo(&remote_path);

    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(fresh, baseline.snapshot);
}

#[test]
fn command_shape_git_config_user_identity_emits_no_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let config_path = baseline.snapshot.identity.git_dir.join("config");

    repo.git(["config", "user.name", "Renamed Tester"]);
    repo.git(["config", "user.email", "renamed@example.com"]);
    let plan = update_from_watch_event_with_no_delta(&mut state, event_for(config_path));

    assert_incremental_refresh_plan(&plan);
    let refresh = refresh_repository_with_plan(
        state.repo(),
        Some(&baseline.snapshot),
        &plan,
        SnapshotOptions::default(),
    )
    .unwrap();
    assert_eq!(refresh.plan, plan);
    assert!(snapshot_update_messages(&mut state, refresh.snapshot).is_empty());
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(fresh, baseline.snapshot);
}

#[test]
fn command_shape_git_commit_allow_empty_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let baseline_oid = baseline.snapshot.head.oid.clone();
    let main_ref_path = baseline
        .snapshot
        .identity
        .common_dir
        .join("refs/heads/main");

    repo.git(["commit", "--allow-empty", "-m", "empty"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(main_ref_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.head_changed);
    assert!(delta.delta.branches_changed);
    assert!(!delta.delta.paths.has_changes());
    assert_eq!(delta.patch.paths, None);
    assert_ne!(delta.patch.head.as_ref().unwrap().oid, baseline_oid);
    assert_ne!(
        delta
            .patch
            .head_commit
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap()
            .parent_oids,
        Vec::<String>::new()
    );

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn opening_nested_path_after_directory_creation_uses_repository_snapshot() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    let nested = repo.path().join("nested/deep");
    fs::create_dir_all(&nested).unwrap();

    let mut state = ProcessState::new(&nested);
    let baseline = subscribe_for_deltas(&mut state);
    let fresh = snapshot_repository(repo.path()).unwrap();

    assert_eq!(baseline.snapshot, fresh);
    assert_eq!(state.repo(), nested.as_path());
    assert_eq!(
        baseline.snapshot.identity.worktree_root.as_deref(),
        Some(repo.path())
    );
}

#[test]
fn opening_linked_worktree_path_uses_linked_worktree_snapshot() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.git(["branch", "side"]);
    let linked_parent = TempDir::new().unwrap();
    let linked_path = linked_parent.path().join("linked");
    repo.git(["worktree", "add", linked_path.to_str().unwrap(), "side"]);

    let mut state = ProcessState::new(&linked_path);
    let baseline = subscribe_for_deltas(&mut state);
    let fresh = snapshot_repository(&linked_path).unwrap();

    assert_eq!(baseline.snapshot, fresh);
    assert_eq!(state.repo(), linked_path.as_path());
    assert!(baseline.snapshot.identity.is_linked_worktree);
    assert_eq!(
        baseline.snapshot.identity.worktree_root.as_deref(),
        Some(linked_path.as_path())
    );
    assert_eq!(baseline.snapshot.head.branch.as_deref(), Some("side"));
}

#[test]
fn opening_bare_repository_path_uses_bare_snapshot() {
    let bare_parent = TempDir::new().unwrap();
    let bare_path = bare_parent.path().join("repo.git");
    init_bare_repo(&bare_path);

    let mut state = ProcessState::new(&bare_path);
    let baseline = subscribe_for_deltas(&mut state);
    let fresh = snapshot_repository(&bare_path).unwrap();

    assert_eq!(baseline.snapshot, fresh);
    assert_eq!(state.repo(), bare_path.as_path());
    assert!(baseline.snapshot.identity.is_bare);
    assert!(baseline.snapshot.identity.worktree_root.is_none());
    assert!(baseline.snapshot.paths.entries.is_empty());
}

#[test]
fn opening_shallow_clone_path_uses_shallow_snapshot() {
    let remote = TestRepo::new();
    remote.write("tracked.txt", "base\n");
    remote.git(["add", "tracked.txt"]);
    remote.git(["commit", "-m", "initial"]);
    remote.write("tracked.txt", "second\n");
    remote.git(["commit", "-am", "second"]);

    let repo = TestRepo::shallow_clone_from(remote.path());
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let fresh = snapshot_repository(repo.path()).unwrap();

    assert_eq!(baseline.snapshot, fresh);
    assert!(baseline.snapshot.identity.is_shallow);
    assert_eq!(baseline.snapshot.head.branch.as_deref(), Some("main"));
}
