use super::*;

#[test]
fn command_shape_git_stash_push_message_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.write("tracked.txt", "work\n");
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let stash_ref_path = baseline.snapshot.identity.git_dir.join("refs/stash");
    assert_eq!(baseline.snapshot.paths.unstaged, vec!["tracked.txt"]);

    repo.git(["stash", "push", "-m", "save work"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(stash_ref_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.paths.has_changes());
    assert!(delta.delta.stashes_changed);
    assert!(delta.patch.paths.as_ref().unwrap().unstaged.is_empty());
    let stashes = delta.patch.stashes.as_ref().unwrap();
    assert_eq!(stashes.len(), 1);
    assert!(stashes[0].message.contains("save work"));

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_stash_push_include_untracked_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.write("untracked.txt", "new\n");
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let stash_ref_path = baseline.snapshot.identity.git_dir.join("refs/stash");
    assert_eq!(baseline.snapshot.paths.untracked, vec!["untracked.txt"]);

    repo.git(["stash", "push", "--include-untracked", "-m", "save all"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(stash_ref_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.paths.has_changes());
    assert!(delta.delta.stashes_changed);
    assert!(delta.patch.paths.as_ref().unwrap().untracked.is_empty());
    let stashes = delta.patch.stashes.as_ref().unwrap();
    assert_eq!(stashes.len(), 1);
    assert!(stashes[0].message.contains("save all"));

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_stash_pop_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.write("tracked.txt", "work\n");
    repo.git(["stash", "push", "-m", "save work"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let stash_ref_path = baseline.snapshot.identity.git_dir.join("refs/stash");
    assert_eq!(baseline.snapshot.stashes.len(), 1);
    assert!(baseline.snapshot.paths.unstaged.is_empty());

    repo.git(["stash", "pop"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(stash_ref_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.paths.has_changes());
    assert!(delta.delta.stashes_changed);
    assert_eq!(
        delta.patch.paths.as_ref().unwrap().unstaged,
        vec!["tracked.txt"]
    );
    assert!(delta.patch.stashes.as_ref().unwrap().is_empty());

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_stash_apply_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.write("tracked.txt", "work\n");
    repo.git(["stash", "push", "-m", "save work"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    assert_eq!(baseline.snapshot.stashes.len(), 1);
    assert!(baseline.snapshot.paths.unstaged.is_empty());

    repo.git(["stash", "apply"]);
    let (plan, delta) =
        update_from_watch_event(&mut state, event_for(repo.path().join("tracked.txt")));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.paths.has_changes());
    assert!(!delta.delta.stashes_changed);
    assert_eq!(
        delta.patch.paths.as_ref().unwrap().unstaged,
        vec!["tracked.txt"]
    );
    assert!(delta.patch.stashes.is_none());

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_stash_drop_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.write("tracked.txt", "work\n");
    repo.git(["stash", "push", "-m", "save work"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let stash_ref_path = baseline.snapshot.identity.git_dir.join("refs/stash");
    assert_eq!(baseline.snapshot.stashes.len(), 1);
    assert!(baseline.snapshot.paths.unstaged.is_empty());

    repo.git(["stash", "drop"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(stash_ref_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(!delta.delta.paths.has_changes());
    assert!(delta.delta.stashes_changed);
    assert!(delta.patch.paths.is_none());
    assert!(delta.patch.stashes.as_ref().unwrap().is_empty());

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_stash_clear_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.write("tracked.txt", "first\n");
    repo.git(["stash", "push", "-m", "first"]);
    repo.write("tracked.txt", "second\n");
    repo.git(["stash", "push", "-m", "second"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let stash_ref_path = baseline.snapshot.identity.git_dir.join("refs/stash");
    assert_eq!(baseline.snapshot.stashes.len(), 2);
    assert!(baseline.snapshot.paths.unstaged.is_empty());

    repo.git(["stash", "clear"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(stash_ref_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(!delta.delta.paths.has_changes());
    assert!(delta.delta.stashes_changed);
    assert!(delta.patch.paths.is_none());
    assert!(delta.patch.stashes.as_ref().unwrap().is_empty());

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_stash_branch_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.write("tracked.txt", "work\n");
    repo.git(["stash", "push", "-m", "save work"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let stash_ref_path = baseline.snapshot.identity.git_dir.join("refs/stash");
    assert_eq!(baseline.snapshot.head.branch.as_deref(), Some("main"));
    assert_eq!(baseline.snapshot.stashes.len(), 1);
    assert!(baseline.snapshot.paths.unstaged.is_empty());

    repo.git(["stash", "branch", "work"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(stash_ref_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.head_changed);
    assert!(delta.delta.paths.has_changes());
    assert!(delta.delta.branches_changed);
    assert!(delta.delta.stashes_changed);
    assert_eq!(
        delta.patch.head.as_ref().unwrap().branch.as_deref(),
        Some("work")
    );
    let head = delta
        .patch
        .branches
        .as_ref()
        .unwrap()
        .iter()
        .find(|branch| branch.is_head)
        .unwrap();
    assert_eq!(head.name, "work");
    assert_eq!(
        delta.patch.paths.as_ref().unwrap().unstaged,
        vec!["tracked.txt"]
    );
    assert!(delta.patch.stashes.as_ref().unwrap().is_empty());

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}
