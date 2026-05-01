use super::*;

#[test]
fn command_shape_edit_tracked_file_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);

    repo.write("tracked.txt", "changed\n");
    let (plan, delta) =
        update_from_watch_event(&mut state, event_for(repo.path().join("tracked.txt")));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert_eq!(delta.delta.paths.unstaged.added, vec!["tracked.txt"]);
    assert_eq!(delta.delta.paths.unstaged.removed, Vec::<String>::new());
    assert!(
        delta
            .delta
            .paths
            .entries_changed
            .contains(&"tracked.txt".to_string())
    );
    assert_eq!(
        delta.patch.paths.as_ref().unwrap().unstaged,
        vec!["tracked.txt"]
    );

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_create_untracked_file_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);

    repo.write("new.txt", "new\n");
    let (plan, delta) = update_from_watch_event(&mut state, event_for(repo.path().join("new.txt")));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert_eq!(delta.delta.paths.untracked.added, vec!["new.txt"]);
    assert_eq!(delta.delta.paths.untracked.removed, Vec::<String>::new());
    assert_eq!(
        delta.patch.paths.as_ref().unwrap().untracked,
        vec!["new.txt"]
    );

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_delete_tracked_file_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);

    fs::remove_file(repo.path().join("tracked.txt")).unwrap();
    let (plan, delta) =
        update_from_watch_event(&mut state, event_for(repo.path().join("tracked.txt")));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert_eq!(delta.delta.paths.unstaged.added, vec!["tracked.txt"]);
    assert_eq!(delta.delta.paths.unstaged.removed, Vec::<String>::new());
    assert!(
        delta
            .delta
            .paths
            .entries_changed
            .contains(&"tracked.txt".to_string())
    );
    assert_eq!(
        delta.patch.paths.as_ref().unwrap().unstaged,
        vec!["tracked.txt"]
    );

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_add_path_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.write("tracked.txt", "changed\n");
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let index_path = baseline.snapshot.identity.git_dir.join("index");

    repo.git(["add", "tracked.txt"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(index_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert_eq!(delta.delta.paths.staged.added, vec!["tracked.txt"]);
    assert_eq!(delta.delta.paths.unstaged.removed, vec!["tracked.txt"]);
    assert!(
        delta
            .delta
            .paths
            .entries_changed
            .contains(&"tracked.txt".to_string())
    );
    let paths = delta.patch.paths.as_ref().unwrap();
    assert_eq!(paths.staged, vec!["tracked.txt"]);
    assert!(paths.unstaged.is_empty());

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_add_all_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.write("tracked.txt", "changed\n");
    repo.write("new.txt", "new\n");
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let index_path = baseline.snapshot.identity.git_dir.join("index");

    repo.git(["add", "-A"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(index_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert_eq!(
        delta.delta.paths.staged.added,
        vec!["new.txt", "tracked.txt"]
    );
    assert_eq!(delta.delta.paths.unstaged.removed, vec!["tracked.txt"]);
    assert_eq!(delta.delta.paths.untracked.removed, vec!["new.txt"]);
    let paths = delta.patch.paths.as_ref().unwrap();
    assert_eq!(paths.staged, vec!["new.txt", "tracked.txt"]);
    assert!(paths.unstaged.is_empty());
    assert!(paths.untracked.is_empty());

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_restore_path_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.write("tracked.txt", "changed\n");
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);

    repo.git(["restore", "tracked.txt"]);
    let (plan, delta) =
        update_from_watch_event(&mut state, event_for(repo.path().join("tracked.txt")));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert_eq!(delta.delta.paths.unstaged.removed, vec!["tracked.txt"]);
    assert!(
        delta
            .delta
            .paths
            .entries_changed
            .contains(&"tracked.txt".to_string())
    );
    let paths = delta.patch.paths.as_ref().unwrap();
    assert!(paths.staged.is_empty());
    assert!(paths.unstaged.is_empty());
    assert!(paths.untracked.is_empty());

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_restore_staged_path_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.write("tracked.txt", "changed\n");
    repo.git(["add", "tracked.txt"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let index_path = baseline.snapshot.identity.git_dir.join("index");

    repo.git(["restore", "--staged", "tracked.txt"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(index_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert_eq!(delta.delta.paths.staged.removed, vec!["tracked.txt"]);
    assert_eq!(delta.delta.paths.unstaged.added, vec!["tracked.txt"]);
    assert!(
        delta
            .delta
            .paths
            .entries_changed
            .contains(&"tracked.txt".to_string())
    );
    let paths = delta.patch.paths.as_ref().unwrap();
    assert!(paths.staged.is_empty());
    assert_eq!(paths.unstaged, vec!["tracked.txt"]);

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_rm_path_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let index_path = baseline.snapshot.identity.git_dir.join("index");

    repo.git(["rm", "tracked.txt"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(index_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert_eq!(delta.delta.paths.staged.added, vec!["tracked.txt"]);
    assert!(
        delta
            .delta
            .paths
            .entries_changed
            .contains(&"tracked.txt".to_string())
    );
    let paths = delta.patch.paths.as_ref().unwrap();
    assert_eq!(paths.staged, vec!["tracked.txt"]);
    assert!(paths.unstaged.is_empty());
    assert!(paths.untracked.is_empty());

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_mv_path_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("old.txt", "base\n");
    repo.git(["add", "old.txt"]);
    repo.git(["commit", "-m", "initial"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let index_path = baseline.snapshot.identity.git_dir.join("index");

    repo.git(["mv", "old.txt", "new.txt"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(index_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert_eq!(delta.delta.paths.staged.added, vec!["old.txt"]);
    assert!(
        delta
            .delta
            .paths
            .entries_changed
            .contains(&"old.txt".to_string())
    );
    let paths = delta.patch.paths.as_ref().unwrap();
    assert_eq!(paths.staged, vec!["old.txt"]);
    assert!(paths.unstaged.is_empty());
    assert!(paths.untracked.is_empty());
    let entry = paths
        .entries
        .iter()
        .find(|entry| entry.path == "old.txt")
        .unwrap();
    assert_eq!(entry.staged_new_path.as_deref(), Some("new.txt"));

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_clean_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.write("untracked.txt", "untracked\n");
    repo.write("scratch/nested.txt", "nested\n");
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);

    repo.git(["clean", "-fd"]);
    let (plan, delta) =
        update_from_watch_event(&mut state, event_for(repo.path().join("untracked.txt")));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert_eq!(
        delta.delta.paths.untracked.removed,
        vec!["scratch/nested.txt", "untracked.txt"]
    );
    let paths = delta.patch.paths.as_ref().unwrap();
    assert!(paths.staged.is_empty());
    assert!(paths.unstaged.is_empty());
    assert!(paths.untracked.is_empty());

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_checkout_path_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.write("tracked.txt", "changed\n");
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);

    repo.git(["checkout", "--", "tracked.txt"]);
    let (plan, delta) =
        update_from_watch_event(&mut state, event_for(repo.path().join("tracked.txt")));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert_eq!(delta.delta.paths.unstaged.removed, vec!["tracked.txt"]);
    assert!(
        delta
            .delta
            .paths
            .entries_changed
            .contains(&"tracked.txt".to_string())
    );
    let paths = delta.patch.paths.as_ref().unwrap();
    assert!(paths.staged.is_empty());
    assert!(paths.unstaged.is_empty());
    assert!(paths.untracked.is_empty());

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_update_index_assume_unchanged_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let index_path = baseline.snapshot.identity.git_dir.join("index");

    repo.git(["update-index", "--assume-unchanged", "tracked.txt"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(index_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert_eq!(delta.delta.paths.entries_changed, vec!["tracked.txt"]);
    let paths = delta.patch.paths.as_ref().unwrap();
    let entry = paths
        .entries
        .iter()
        .find(|entry| entry.path == "tracked.txt")
        .unwrap();
    assert!(entry.status.assume_unchanged);
    assert!(!entry.status.skip_worktree);

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_update_index_no_assume_unchanged_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.git(["update-index", "--assume-unchanged", "tracked.txt"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let index_path = baseline.snapshot.identity.git_dir.join("index");

    repo.git(["update-index", "--no-assume-unchanged", "tracked.txt"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(index_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert_eq!(delta.delta.paths.entries_changed, vec!["tracked.txt"]);
    let paths = delta.patch.paths.as_ref().unwrap();
    assert!(paths.entries.is_empty());

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_update_index_skip_worktree_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let index_path = baseline.snapshot.identity.git_dir.join("index");

    repo.git(["update-index", "--skip-worktree", "tracked.txt"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(index_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert_eq!(delta.delta.paths.entries_changed, vec!["tracked.txt"]);
    let paths = delta.patch.paths.as_ref().unwrap();
    let entry = paths
        .entries
        .iter()
        .find(|entry| entry.path == "tracked.txt")
        .unwrap();
    assert!(!entry.status.assume_unchanged);
    assert!(entry.status.skip_worktree);

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_update_index_no_skip_worktree_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.git(["update-index", "--skip-worktree", "tracked.txt"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let index_path = baseline.snapshot.identity.git_dir.join("index");

    repo.git(["update-index", "--no-skip-worktree", "tracked.txt"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(index_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert_eq!(delta.delta.paths.entries_changed, vec!["tracked.txt"]);
    let paths = delta.patch.paths.as_ref().unwrap();
    assert!(paths.entries.is_empty());

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}
