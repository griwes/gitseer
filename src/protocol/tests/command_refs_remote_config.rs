use super::*;

#[test]
fn command_shape_create_local_bare_remote_repository_emits_no_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let remotes_dir = TempDir::new().unwrap();
    let remote_path = remotes_dir.path().join("origin.git");

    init_bare_repo(&remote_path);

    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(fresh, baseline.snapshot);
}

#[test]
fn command_shape_git_remote_add_emits_patchable_delta() {
    let remotes_dir = TempDir::new().unwrap();
    let remote_path = remotes_dir.path().join("origin.git");
    init_bare_repo(&remote_path);

    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let config_path = baseline.snapshot.identity.git_dir.join("config");

    repo.git(["remote", "add", "origin", remote_path.to_str().unwrap()]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(config_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.remotes_changed);
    let remotes = delta.patch.remotes.as_ref().unwrap();
    assert_eq!(remotes.len(), 1);
    assert_eq!(remotes[0].name, "origin");
    assert_eq!(remotes[0].url.as_deref(), remote_path.to_str());

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_remote_rename_emits_patchable_delta() {
    let remotes_dir = TempDir::new().unwrap();
    let remote_path = remotes_dir.path().join("origin.git");
    init_bare_repo(&remote_path);

    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.git(["remote", "add", "origin", remote_path.to_str().unwrap()]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let config_path = baseline.snapshot.identity.git_dir.join("config");

    repo.git(["remote", "rename", "origin", "upstream"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(config_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.remotes_changed);
    let remotes = delta.patch.remotes.as_ref().unwrap();
    assert_eq!(remotes.len(), 1);
    assert_eq!(remotes[0].name, "upstream");
    assert!(!remotes.iter().any(|remote| remote.name == "origin"));

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_remote_remove_emits_patchable_delta() {
    let remotes_dir = TempDir::new().unwrap();
    let remote_path = remotes_dir.path().join("origin.git");
    init_bare_repo(&remote_path);

    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.git(["remote", "add", "origin", remote_path.to_str().unwrap()]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let config_path = baseline.snapshot.identity.git_dir.join("config");

    repo.git(["remote", "remove", "origin"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(config_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.remotes_changed);
    assert!(delta.patch.remotes.as_ref().unwrap().is_empty());

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_remote_set_url_emits_patchable_delta() {
    let remote_parent = TempDir::new().unwrap();
    let old_remote_path = remote_parent.path().join("old.git");
    let new_remote_path = remote_parent.path().join("new.git");
    init_bare_repo(&old_remote_path);
    init_bare_repo(&new_remote_path);
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.git(["remote", "add", "origin", old_remote_path.to_str().unwrap()]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let config_path = baseline.snapshot.identity.git_dir.join("config");

    repo.git([
        "remote",
        "set-url",
        "origin",
        new_remote_path.to_str().unwrap(),
    ]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(config_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.remotes_changed);
    assert!(!delta.delta.paths.has_changes());
    let remotes = delta.patch.remotes.as_ref().unwrap();
    assert_eq!(remotes[0].name, "origin");
    assert_eq!(
        remotes[0].url.as_deref(),
        Some(new_remote_path.to_str().unwrap())
    );

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_remote_set_head_auto_emits_patchable_delta() {
    let remote_parent = TempDir::new().unwrap();
    let remote_path = remote_parent.path().join("remote.git");
    init_bare_repo(&remote_path);
    let repo = TestRepo::clone_from(&remote_path);
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.git(["push", "-u", "origin", "main"]);
    repo.git(["remote", "set-head", "origin", "-d"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let remote_head_path = baseline
        .snapshot
        .identity
        .git_dir
        .join("refs/remotes/origin/HEAD");
    assert!(
        !baseline
            .snapshot
            .branches
            .iter()
            .any(|branch| branch.name == "origin/HEAD")
    );

    repo.git(["remote", "set-head", "origin", "--auto"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(remote_head_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.branches_changed);
    assert!(
        delta
            .patch
            .branches
            .as_ref()
            .unwrap()
            .iter()
            .any(|branch| branch.name == "origin/HEAD")
    );

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_direct_remote_config_edit_emits_patchable_delta() {
    let remote_parent = TempDir::new().unwrap();
    let old_remote_path = remote_parent.path().join("old.git");
    let new_remote_path = remote_parent.path().join("new.git");
    init_bare_repo(&old_remote_path);
    init_bare_repo(&new_remote_path);
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.git(["remote", "add", "origin", old_remote_path.to_str().unwrap()]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let config_path = baseline.snapshot.identity.git_dir.join("config");

    let config = fs::read_to_string(&config_path).unwrap();
    fs::write(
        &config_path,
        config.replace(
            old_remote_path.to_str().unwrap(),
            new_remote_path.to_str().unwrap(),
        ),
    )
    .unwrap();
    let (plan, delta) = update_from_watch_event(&mut state, event_for(config_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.remotes_changed);
    assert!(!delta.delta.paths.has_changes());
    assert_eq!(
        delta.patch.remotes.as_ref().unwrap()[0].url.as_deref(),
        Some(new_remote_path.to_str().unwrap())
    );

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}
