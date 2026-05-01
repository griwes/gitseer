use super::*;

#[test]
fn command_shape_add_root_gitignore_pattern_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.write("build/output.log", "artifact\n");
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);

    repo.write(".gitignore", "build/\n");
    let (plan, delta) =
        update_from_watch_event(&mut state, event_for(repo.path().join(".gitignore")));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert_eq!(delta.delta.paths.untracked.added, vec![".gitignore"]);
    assert_eq!(
        delta.delta.paths.untracked.removed,
        vec!["build/output.log"]
    );
    let paths = delta.patch.paths.as_ref().unwrap();
    assert_eq!(paths.untracked, vec![".gitignore"]);

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_modify_root_gitignore_pattern_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.write(".gitignore", "build/\n");
    repo.git(["add", "tracked.txt", ".gitignore"]);
    repo.git(["commit", "-m", "initial"]);
    repo.write("build/output.log", "build\n");
    repo.write("cache/output.log", "cache\n");
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);

    repo.write(".gitignore", "cache/\n");
    let (plan, delta) =
        update_from_watch_event(&mut state, event_for(repo.path().join(".gitignore")));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert_eq!(delta.delta.paths.unstaged.added, vec![".gitignore"]);
    assert_eq!(delta.delta.paths.untracked.added, vec!["build/output.log"]);
    assert_eq!(
        delta.delta.paths.untracked.removed,
        vec!["cache/output.log"]
    );
    let paths = delta.patch.paths.as_ref().unwrap();
    assert_eq!(paths.unstaged, vec![".gitignore"]);
    assert_eq!(paths.untracked, vec!["build/output.log"]);

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_remove_root_gitignore_pattern_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.write(".gitignore", "build/\n");
    repo.git(["add", "tracked.txt", ".gitignore"]);
    repo.git(["commit", "-m", "initial"]);
    repo.write("build/output.log", "build\n");
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);

    fs::remove_file(repo.path().join(".gitignore")).unwrap();
    let (plan, delta) =
        update_from_watch_event(&mut state, event_for(repo.path().join(".gitignore")));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert_eq!(delta.delta.paths.unstaged.added, vec![".gitignore"]);
    assert_eq!(delta.delta.paths.untracked.added, vec!["build/output.log"]);
    let paths = delta.patch.paths.as_ref().unwrap();
    assert_eq!(paths.unstaged, vec![".gitignore"]);
    assert_eq!(paths.untracked, vec!["build/output.log"]);

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_add_nested_gitignore_pattern_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.write("module/output.log", "artifact\n");
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);

    repo.write("module/.gitignore", "*.log\n");
    let (plan, delta) =
        update_from_watch_event(&mut state, event_for(repo.path().join("module/.gitignore")));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert_eq!(delta.delta.paths.untracked.added, vec!["module/.gitignore"]);
    assert_eq!(
        delta.delta.paths.untracked.removed,
        vec!["module/output.log"]
    );
    let paths = delta.patch.paths.as_ref().unwrap();
    assert_eq!(paths.untracked, vec!["module/.gitignore"]);

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_modify_git_info_exclude_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.write("build/output.log", "artifact\n");
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let exclude_path = baseline.snapshot.identity.git_dir.join("info/exclude");

    fs::write(&exclude_path, "build/\n").unwrap();
    let (plan, delta) = update_from_watch_event(&mut state, event_for(exclude_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert_eq!(
        delta.delta.paths.untracked.removed,
        vec!["build/output.log"]
    );
    let paths = delta.patch.paths.as_ref().unwrap();
    assert!(paths.untracked.is_empty());

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_modify_core_excludesfile_emits_patchable_delta() {
    let excludes_dir = TempDir::new().unwrap();
    let excludes_path = excludes_dir.path().join("global-ignore");
    fs::write(&excludes_path, "").unwrap();

    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.git([
        "config",
        "core.excludesfile",
        excludes_path.to_str().unwrap(),
    ]);
    repo.write("build/output.log", "artifact\n");
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);

    fs::write(&excludes_path, "build/\n").unwrap();
    let (plan, delta) = update_from_watch_event(&mut state, event_for(excludes_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert_eq!(
        delta.delta.paths.untracked.removed,
        vec!["build/output.log"]
    );
    let paths = delta.patch.paths.as_ref().unwrap();
    assert!(paths.untracked.is_empty());

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_create_ignored_build_file_emits_no_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.write(".gitignore", "build/\n");
    repo.git(["add", "tracked.txt", ".gitignore"]);
    repo.git(["commit", "-m", "initial"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);

    repo.write("build/output.log", "artifact\n");
    let plan = update_from_watch_event_with_no_delta(
        &mut state,
        event_for(repo.path().join("build/output.log")),
    );

    assert_eq!(plan, RefreshPlan::None);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(fresh, baseline.snapshot);
}

#[test]
fn command_shape_modify_ignored_build_file_emits_no_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.write(".gitignore", "build/\n");
    repo.git(["add", "tracked.txt", ".gitignore"]);
    repo.git(["commit", "-m", "initial"]);
    repo.write("build/output.log", "artifact\n");
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);

    repo.write("build/output.log", "changed\n");
    let plan = update_from_watch_event_with_no_delta(
        &mut state,
        event_for(repo.path().join("build/output.log")),
    );

    assert_eq!(plan, RefreshPlan::None);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(fresh, baseline.snapshot);
}

#[test]
fn command_shape_delete_ignored_build_file_emits_no_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.write(".gitignore", "build/\n");
    repo.git(["add", "tracked.txt", ".gitignore"]);
    repo.git(["commit", "-m", "initial"]);
    repo.write("build/output.log", "artifact\n");
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);

    fs::remove_file(repo.path().join("build/output.log")).unwrap();
    let plan = update_from_watch_event_with_no_delta(
        &mut state,
        event_for(repo.path().join("build/output.log")),
    );

    assert_eq!(plan, RefreshPlan::None);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(fresh, baseline.snapshot);
}

#[test]
fn command_shape_git_add_force_ignored_path_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.write(".gitignore", "build/\n");
    repo.git(["add", "tracked.txt", ".gitignore"]);
    repo.git(["commit", "-m", "initial"]);
    repo.write("build/output.log", "artifact\n");
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let index_path = baseline.snapshot.identity.git_dir.join("index");

    repo.git(["add", "-f", "build/output.log"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(index_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert_eq!(delta.delta.paths.staged.added, vec!["build/output.log"]);
    let paths = delta.patch.paths.as_ref().unwrap();
    assert_eq!(paths.staged, vec!["build/output.log"]);
    let entry = paths
        .entries
        .iter()
        .find(|entry| entry.path == "build/output.log")
        .unwrap();
    assert!(entry.status.index_new);

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_unignore_path_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.write(".gitignore", "build/\n");
    repo.git(["add", "tracked.txt", ".gitignore"]);
    repo.git(["commit", "-m", "initial"]);
    repo.write("build/output.log", "artifact\n");
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);

    repo.write(".gitignore", "");
    let (plan, delta) =
        update_from_watch_event(&mut state, event_for(repo.path().join(".gitignore")));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert_eq!(delta.delta.paths.unstaged.added, vec![".gitignore"]);
    assert_eq!(delta.delta.paths.untracked.added, vec!["build/output.log"]);
    let paths = delta.patch.paths.as_ref().unwrap();
    assert_eq!(paths.unstaged, vec![".gitignore"]);
    assert_eq!(paths.untracked, vec!["build/output.log"]);

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}
