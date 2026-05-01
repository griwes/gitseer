use super::*;

#[test]
fn command_shape_git_worktree_add_branch_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.git(["branch", "side"]);
    let linked_parent = TempDir::new().unwrap();
    let linked_path = linked_parent.path().join("linked");
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let worktree_metadata_path = baseline
        .snapshot
        .identity
        .common_dir
        .join("worktrees/linked/gitdir");

    repo.git(["worktree", "add", linked_path.to_str().unwrap(), "side"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(worktree_metadata_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(!delta.delta.identity_changed);
    assert!(delta.delta.worktrees_changed);
    let worktrees = delta.patch.worktrees.as_ref().unwrap();
    assert_eq!(worktrees.len(), 1);
    assert_eq!(worktrees[0].name, "linked");
    assert_eq!(worktrees[0].path, linked_path);
    assert!(!worktrees[0].locked);

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_worktree_add_new_branch_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    let linked_parent = TempDir::new().unwrap();
    let linked_path = linked_parent.path().join("linked");
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let worktree_metadata_path = baseline
        .snapshot
        .identity
        .common_dir
        .join("worktrees/linked/gitdir");

    repo.git([
        "worktree",
        "add",
        "-b",
        "feature",
        linked_path.to_str().unwrap(),
    ]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(worktree_metadata_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(!delta.delta.identity_changed);
    assert!(delta.delta.branches_changed);
    assert!(delta.delta.worktrees_changed);
    assert!(
        delta
            .patch
            .branches
            .as_ref()
            .unwrap()
            .iter()
            .any(|branch| branch.name == "feature")
    );
    let worktrees = delta.patch.worktrees.as_ref().unwrap();
    assert_eq!(worktrees.len(), 1);
    assert_eq!(worktrees[0].name, "linked");
    assert_eq!(worktrees[0].path, linked_path);

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_worktree_remove_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.git(["branch", "side"]);
    let linked_parent = TempDir::new().unwrap();
    let linked_path = linked_parent.path().join("linked");
    repo.git(["worktree", "add", linked_path.to_str().unwrap(), "side"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let worktree_metadata_path = baseline
        .snapshot
        .identity
        .common_dir
        .join("worktrees/linked/gitdir");
    assert_eq!(baseline.snapshot.worktrees.len(), 1);

    repo.git(["worktree", "remove", linked_path.to_str().unwrap()]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(worktree_metadata_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(!delta.delta.identity_changed);
    assert!(!delta.delta.branches_changed);
    assert!(delta.delta.worktrees_changed);
    assert!(delta.patch.branches.is_none());
    assert!(delta.patch.worktrees.as_ref().unwrap().is_empty());

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_worktree_prune_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.git(["branch", "side"]);
    let linked_parent = TempDir::new().unwrap();
    let linked_path = linked_parent.path().join("linked");
    repo.git(["worktree", "add", linked_path.to_str().unwrap(), "side"]);
    fs::remove_dir_all(&linked_path).unwrap();
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let worktree_metadata_path = baseline
        .snapshot
        .identity
        .common_dir
        .join("worktrees/linked/gitdir");
    assert_eq!(baseline.snapshot.worktrees.len(), 1);

    repo.git(["worktree", "prune"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(worktree_metadata_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.worktrees_changed);
    assert!(delta.patch.worktrees.as_ref().unwrap().is_empty());

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_linked_worktree_branch_commit_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.git(["branch", "side"]);
    let linked_parent = TempDir::new().unwrap();
    let linked_path = linked_parent.path().join("linked");
    repo.git(["worktree", "add", linked_path.to_str().unwrap(), "side"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let side_ref_path = baseline
        .snapshot
        .identity
        .common_dir
        .join("refs/heads/side");
    let baseline_side = baseline
        .snapshot
        .branches
        .iter()
        .find(|branch| branch.name == "side")
        .unwrap()
        .oid
        .clone();

    fs::write(linked_path.join("linked.txt"), "linked\n").unwrap();
    git_in(&linked_path, ["add", "linked.txt"]);
    git_in(&linked_path, ["commit", "-m", "linked"]);
    let linked_oid = git_stdout_in(&linked_path, ["rev-parse", "side"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(side_ref_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(!delta.delta.head_changed);
    assert!(delta.delta.branches_changed);
    assert_ne!(baseline_side.as_deref(), Some(linked_oid.as_str()));
    let side = delta
        .patch
        .branches
        .as_ref()
        .unwrap()
        .iter()
        .find(|branch| branch.name == "side")
        .unwrap();
    assert_eq!(side.oid.as_deref(), Some(linked_oid.as_str()));

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_worktree_lock_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.git(["branch", "side"]);
    let linked_parent = TempDir::new().unwrap();
    let linked_path = linked_parent.path().join("linked");
    repo.git(["worktree", "add", linked_path.to_str().unwrap(), "side"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let lock_path = baseline
        .snapshot
        .identity
        .common_dir
        .join("worktrees/linked/locked");
    assert!(!baseline.snapshot.worktrees[0].locked);

    repo.git([
        "worktree",
        "lock",
        "--reason",
        "testing",
        linked_path.to_str().unwrap(),
    ]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(lock_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.worktrees_changed);
    let worktree = &delta.patch.worktrees.as_ref().unwrap()[0];
    assert!(worktree.locked);
    assert_eq!(worktree.lock_reason.as_deref(), Some("testing"));

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_worktree_unlock_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.git(["branch", "side"]);
    let linked_parent = TempDir::new().unwrap();
    let linked_path = linked_parent.path().join("linked");
    repo.git(["worktree", "add", linked_path.to_str().unwrap(), "side"]);
    repo.git([
        "worktree",
        "lock",
        "--reason",
        "testing",
        linked_path.to_str().unwrap(),
    ]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let lock_path = baseline
        .snapshot
        .identity
        .common_dir
        .join("worktrees/linked/locked");
    assert!(baseline.snapshot.worktrees[0].locked);

    repo.git(["worktree", "unlock", linked_path.to_str().unwrap()]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(lock_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.worktrees_changed);
    let worktree = &delta.patch.worktrees.as_ref().unwrap()[0];
    assert!(!worktree.locked);
    assert!(worktree.lock_reason.is_none());

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}
