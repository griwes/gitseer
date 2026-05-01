use super::*;

#[test]
fn command_shape_create_local_submodule_repository_emits_no_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);

    let submodule_repo = TestRepo::new();
    submodule_repo.write("README.md", "submodule\n");
    submodule_repo.git(["add", "README.md"]);
    submodule_repo.git(["commit", "-m", "submodule initial"]);

    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(fresh, baseline.snapshot);
}

#[test]
fn command_shape_git_submodule_add_emits_patchable_delta() {
    let submodule_repo = TestRepo::new();
    submodule_repo.write("README.md", "submodule\n");
    submodule_repo.git(["add", "README.md"]);
    submodule_repo.git(["commit", "-m", "submodule initial"]);
    let submodule_url = submodule_repo.path().to_str().unwrap();

    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);

    repo.git_allow_file_protocol(["submodule", "add", submodule_url, "deps/sub"]);
    let (plan, delta) =
        update_from_watch_event(&mut state, event_for(repo.path().join(".gitmodules")));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.paths.has_changes());
    assert!(delta.delta.submodules_changed);
    assert!(
        delta
            .patch
            .paths
            .as_ref()
            .unwrap()
            .staged
            .contains(&".gitmodules".to_string())
    );
    let submodules = delta.patch.submodules.as_ref().unwrap();
    assert_eq!(submodules.len(), 1);
    let submodule = &submodules[0];
    assert_eq!(submodule.name, "deps/sub");
    assert_eq!(submodule.path, PathBuf::from("deps/sub"));
    assert_eq!(submodule.url.as_deref(), Some(submodule_url));
    assert!(submodule.status.in_config);
    assert!(submodule.status.in_index);
    assert!(submodule.status.in_workdir);
    assert!(submodule.status.index_added);
    assert!(!submodule.status.in_head);

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_submodule_update_init_emits_patchable_delta() {
    let submodule_repo = TestRepo::new();
    submodule_repo.write("README.md", "submodule\n");
    submodule_repo.git(["add", "README.md"]);
    submodule_repo.git(["commit", "-m", "submodule initial"]);
    let submodule_url = submodule_repo.path().to_str().unwrap();

    let super_repo = TestRepo::new();
    super_repo.write("tracked.txt", "base\n");
    super_repo.git(["add", "tracked.txt"]);
    super_repo.git(["commit", "-m", "initial"]);
    super_repo.git_allow_file_protocol(["submodule", "add", submodule_url, "deps/sub"]);
    super_repo.git(["commit", "-am", "add submodule"]);

    let repo = TestRepo::clone_from(super_repo.path());
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let baseline_submodule = baseline
        .snapshot
        .submodules
        .iter()
        .find(|submodule| submodule.name == "deps/sub")
        .unwrap();
    assert!(baseline_submodule.status.workdir_uninitialized);

    repo.git_allow_file_protocol(["submodule", "update", "--init"]);
    let (plan, delta) =
        update_from_watch_event(&mut state, event_for(repo.path().join("deps/sub/.git")));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.submodules_changed);
    let submodule = delta
        .patch
        .submodules
        .as_ref()
        .unwrap()
        .iter()
        .find(|submodule| submodule.name == "deps/sub")
        .unwrap();
    assert!(submodule.status.in_workdir);
    assert!(!submodule.status.workdir_uninitialized);
    assert!(submodule.workdir_oid.is_some());

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_submodule_commit_emits_parent_patchable_delta() {
    let submodule_repo = TestRepo::new();
    submodule_repo.write("README.md", "submodule\n");
    submodule_repo.git(["add", "README.md"]);
    submodule_repo.git(["commit", "-m", "submodule initial"]);
    let submodule_url = submodule_repo.path().to_str().unwrap();

    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.git_allow_file_protocol(["submodule", "add", submodule_url, "deps/sub"]);
    repo.git(["commit", "-am", "add submodule"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let baseline_submodule = baseline
        .snapshot
        .submodules
        .iter()
        .find(|submodule| submodule.name == "deps/sub")
        .unwrap();
    let baseline_workdir_oid = baseline_submodule.workdir_oid.clone();

    let submodule_path = repo.path().join("deps/sub");
    git_in(&submodule_path, ["config", "commit.gpgsign", "false"]);
    git_in(&submodule_path, ["config", "tag.gpgsign", "false"]);
    fs::write(submodule_path.join("README.md"), "submodule changed\n").unwrap();
    git_in(&submodule_path, ["add", "README.md"]);
    git_in(&submodule_path, ["commit", "-m", "submodule change"]);
    let (plan, delta) =
        update_from_watch_event(&mut state, event_for(submodule_path.join("README.md")));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.paths.has_changes());
    assert!(delta.delta.submodules_changed);
    let submodule = delta
        .patch
        .submodules
        .as_ref()
        .unwrap()
        .iter()
        .find(|submodule| submodule.name == "deps/sub")
        .unwrap();
    assert_ne!(submodule.workdir_oid, baseline_workdir_oid);
    assert!(submodule.status.workdir_modified);

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_submodule_deinit_emits_patchable_delta() {
    let submodule_repo = TestRepo::new();
    submodule_repo.write("README.md", "submodule\n");
    submodule_repo.git(["add", "README.md"]);
    submodule_repo.git(["commit", "-m", "submodule initial"]);
    let submodule_url = submodule_repo.path().to_str().unwrap();

    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.git_allow_file_protocol(["submodule", "add", submodule_url, "deps/sub"]);
    repo.git(["commit", "-am", "add submodule"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let baseline_submodule = baseline
        .snapshot
        .submodules
        .iter()
        .find(|submodule| submodule.name == "deps/sub")
        .unwrap();
    assert!(baseline_submodule.status.in_workdir);

    repo.git(["submodule", "deinit", "-f", "deps/sub"]);
    let (plan, delta) =
        update_from_watch_event(&mut state, event_for(repo.path().join("deps/sub")));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.submodules_changed);
    let submodule = delta
        .patch
        .submodules
        .as_ref()
        .unwrap()
        .iter()
        .find(|submodule| submodule.name == "deps/sub")
        .unwrap();
    assert!(!submodule.status.in_workdir);
    assert!(submodule.status.workdir_uninitialized);

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_submodule_set_url_emits_patchable_delta() {
    let submodule_repo = TestRepo::new();
    submodule_repo.write("README.md", "submodule\n");
    submodule_repo.git(["add", "README.md"]);
    submodule_repo.git(["commit", "-m", "submodule initial"]);
    let replacement_repo = TestRepo::new();
    replacement_repo.write("README.md", "replacement\n");
    replacement_repo.git(["add", "README.md"]);
    replacement_repo.git(["commit", "-m", "replacement initial"]);
    let replacement_url = replacement_repo.path().to_str().unwrap();

    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.git_allow_file_protocol([
        "submodule",
        "add",
        submodule_repo.path().to_str().unwrap(),
        "deps/sub",
    ]);
    repo.git(["commit", "-am", "add submodule"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);

    repo.git(["submodule", "set-url", "deps/sub", replacement_url]);
    let (plan, delta) =
        update_from_watch_event(&mut state, event_for(repo.path().join(".gitmodules")));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.paths.has_changes());
    assert!(delta.delta.submodules_changed);
    let submodule = delta
        .patch
        .submodules
        .as_ref()
        .unwrap()
        .iter()
        .find(|submodule| submodule.name == "deps/sub")
        .unwrap();
    assert_eq!(submodule.url.as_deref(), Some(replacement_url));

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_submodule_set_branch_emits_patchable_delta() {
    let submodule_repo = TestRepo::new();
    submodule_repo.write("README.md", "submodule\n");
    submodule_repo.git(["add", "README.md"]);
    submodule_repo.git(["commit", "-m", "submodule initial"]);
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.git_allow_file_protocol([
        "submodule",
        "add",
        submodule_repo.path().to_str().unwrap(),
        "deps/sub",
    ]);
    repo.git(["commit", "-am", "add submodule"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);

    repo.git(["submodule", "set-branch", "--branch", "stable", "deps/sub"]);
    let (plan, delta) =
        update_from_watch_event(&mut state, event_for(repo.path().join(".gitmodules")));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.paths.has_changes());
    assert!(delta.delta.submodules_changed);
    let submodule = delta
        .patch
        .submodules
        .as_ref()
        .unwrap()
        .iter()
        .find(|submodule| submodule.name == "deps/sub")
        .unwrap();
    assert_eq!(submodule.branch.as_deref(), Some("stable"));

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_submodule_sync_emits_no_semantic_delta() {
    let submodule_repo = TestRepo::new();
    submodule_repo.write("README.md", "submodule\n");
    submodule_repo.git(["add", "README.md"]);
    submodule_repo.git(["commit", "-m", "submodule initial"]);
    let replacement_repo = TestRepo::new();
    replacement_repo.write("README.md", "replacement\n");
    replacement_repo.git(["add", "README.md"]);
    replacement_repo.git(["commit", "-m", "replacement initial"]);
    let replacement_url = replacement_repo.path().to_str().unwrap();

    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.git_allow_file_protocol([
        "submodule",
        "add",
        submodule_repo.path().to_str().unwrap(),
        "deps/sub",
    ]);
    repo.git(["commit", "-am", "add submodule"]);
    let gitmodules = repo.path().join(".gitmodules");
    let contents = fs::read_to_string(&gitmodules).unwrap();
    fs::write(
        &gitmodules,
        contents.replace(submodule_repo.path().to_str().unwrap(), replacement_url),
    )
    .unwrap();
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    assert_eq!(
        baseline.snapshot.submodules[0].url.as_deref(),
        Some(replacement_url)
    );
    let config_path = baseline.snapshot.identity.git_dir.join("config");

    repo.git(["submodule", "sync"]);
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
fn command_shape_nested_submodule_change_emits_parent_patchable_delta() {
    let nested_repo = TestRepo::new();
    nested_repo.write("README.md", "nested\n");
    nested_repo.git(["add", "README.md"]);
    nested_repo.git(["commit", "-m", "nested initial"]);

    let submodule_repo = TestRepo::new();
    submodule_repo.write("README.md", "submodule\n");
    submodule_repo.git(["add", "README.md"]);
    submodule_repo.git(["commit", "-m", "submodule initial"]);
    submodule_repo.git_allow_file_protocol([
        "submodule",
        "add",
        nested_repo.path().to_str().unwrap(),
        "deps/nested",
    ]);
    submodule_repo.git(["commit", "-am", "add nested submodule"]);

    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.git_allow_file_protocol([
        "submodule",
        "add",
        submodule_repo.path().to_str().unwrap(),
        "deps/sub",
    ]);
    repo.git(["commit", "-am", "add submodule"]);
    let submodule_path = repo.path().join("deps/sub");
    git_allow_file_protocol_in(&submodule_path, ["submodule", "update", "--init"]);
    let nested_path = submodule_path.join("deps/nested");
    git_in(&nested_path, ["config", "commit.gpgsign", "false"]);
    git_in(&nested_path, ["config", "tag.gpgsign", "false"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let baseline_submodule = baseline
        .snapshot
        .submodules
        .iter()
        .find(|submodule| submodule.name == "deps/sub")
        .cloned()
        .unwrap();
    assert!(!baseline_submodule.status.workdir_modified);

    fs::write(nested_path.join("README.md"), "nested changed\n").unwrap();
    git_in(&nested_path, ["add", "README.md"]);
    git_in(&nested_path, ["commit", "-m", "nested change"]);
    let (plan, delta) =
        update_from_watch_event(&mut state, event_for(nested_path.join("README.md")));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.submodules_changed);
    let submodule = delta
        .patch
        .submodules
        .as_ref()
        .unwrap()
        .iter()
        .find(|submodule| submodule.name == "deps/sub")
        .unwrap();
    assert_ne!(submodule, &baseline_submodule);
    assert!(
        submodule.status.workdir_modified
            || submodule.status.workdir_worktree_modified
            || submodule.status.workdir_untracked
    );

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}
