use super::*;

#[test]
fn command_shape_git_push_set_upstream_emits_patchable_delta() {
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

    repo.git(["push", "-u", "origin", "main"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(config_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.upstream_changed);
    assert!(delta.delta.branches_changed);
    let upstream = delta.patch.upstream.as_ref().unwrap().as_ref().unwrap();
    assert_eq!(upstream.name, "origin/main");
    assert_eq!(upstream.ahead, 0);
    assert_eq!(upstream.behind, 0);
    let branches = delta.patch.branches.as_ref().unwrap();
    assert!(branches.iter().any(|branch| {
        branch.name == "origin/main" && branch.kind == crate::BranchKind::Remote
    }));
    let head = branches.iter().find(|branch| branch.is_head).unwrap();
    assert_eq!(head.upstream.as_deref(), Some("origin/main"));
    assert_eq!(head.upstream_ahead, Some(0));
    assert_eq!(head.upstream_behind, Some(0));

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_branch_set_upstream_to_emits_patchable_delta() {
    let remotes_dir = TempDir::new().unwrap();
    let remote_path = remotes_dir.path().join("origin.git");
    init_bare_repo(&remote_path);

    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.git(["remote", "add", "origin", remote_path.to_str().unwrap()]);
    repo.git(["push", "origin", "main"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let config_path = baseline.snapshot.identity.git_dir.join("config");
    assert!(baseline.snapshot.upstream.is_none());
    assert!(baseline.snapshot.branches.iter().any(|branch| {
        branch.name == "origin/main" && branch.kind == crate::BranchKind::Remote
    }));

    repo.git(["branch", "--set-upstream-to=origin/main", "main"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(config_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.upstream_changed);
    assert!(delta.delta.branches_changed);
    let upstream = delta.patch.upstream.as_ref().unwrap().as_ref().unwrap();
    assert_eq!(upstream.name, "origin/main");
    assert_eq!(upstream.ahead, 0);
    assert_eq!(upstream.behind, 0);
    let branches = delta.patch.branches.as_ref().unwrap();
    let head = branches.iter().find(|branch| branch.is_head).unwrap();
    assert_eq!(head.upstream.as_deref(), Some("origin/main"));
    assert_eq!(head.upstream_ahead, Some(0));
    assert_eq!(head.upstream_behind, Some(0));

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_branch_unset_upstream_emits_patchable_delta() {
    let remotes_dir = TempDir::new().unwrap();
    let remote_path = remotes_dir.path().join("origin.git");
    init_bare_repo(&remote_path);

    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.git(["remote", "add", "origin", remote_path.to_str().unwrap()]);
    repo.git(["push", "-u", "origin", "main"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let config_path = baseline.snapshot.identity.git_dir.join("config");
    assert!(baseline.snapshot.upstream.is_some());

    repo.git(["branch", "--unset-upstream"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(config_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.upstream_changed);
    assert!(delta.delta.branches_changed);
    assert!(delta.patch.upstream.as_ref().unwrap().is_none());
    let branches = delta.patch.branches.as_ref().unwrap();
    let head = branches.iter().find(|branch| branch.is_head).unwrap();
    assert!(head.upstream.is_none());
    assert!(head.upstream_ahead.is_none());
    assert!(head.upstream_behind.is_none());

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_fetch_remote_emits_patchable_delta() {
    let remote = TestRepo::new();
    remote.write("remote.txt", "remote\n");
    remote.git(["add", "remote.txt"]);
    remote.git(["commit", "-m", "remote"]);

    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.git(["remote", "add", "origin", remote.path().to_str().unwrap()]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let fetch_head_path = baseline.snapshot.identity.git_dir.join("FETCH_HEAD");
    assert!(
        !baseline
            .snapshot
            .branches
            .iter()
            .any(|branch| branch.name == "origin/main")
    );

    repo.git(["fetch", "origin"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(fetch_head_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(!delta.delta.upstream_changed);
    assert!(delta.delta.branches_changed);
    assert!(delta.delta.remotes_changed);
    let origin = delta
        .patch
        .remotes
        .as_ref()
        .unwrap()
        .iter()
        .find(|remote| remote.name == "origin")
        .unwrap();
    assert_eq!(origin.default_branch.as_deref(), Some("main"));
    let branches = delta.patch.branches.as_ref().unwrap();
    assert!(branches.iter().any(|branch| {
        branch.name == "origin/main" && branch.kind == crate::BranchKind::Remote
    }));

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_fetch_prune_remote_emits_patchable_delta() {
    let remote = TestRepo::new();
    remote.write("remote.txt", "remote\n");
    remote.git(["add", "remote.txt"]);
    remote.git(["commit", "-m", "remote"]);
    remote.git(["branch", "side"]);

    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.git(["remote", "add", "origin", remote.path().to_str().unwrap()]);
    repo.git(["fetch", "origin"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let pruned_ref_path = baseline
        .snapshot
        .identity
        .git_dir
        .join("refs/remotes/origin/side");
    assert!(
        baseline
            .snapshot
            .branches
            .iter()
            .any(|branch| branch.name == "origin/side")
    );

    remote.git(["branch", "-d", "side"]);
    repo.git(["fetch", "--prune", "origin"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(pruned_ref_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(!delta.delta.upstream_changed);
    assert!(delta.delta.branches_changed);
    let branches = delta.patch.branches.as_ref().unwrap();
    assert!(branches.iter().any(|branch| {
        branch.name == "origin/main" && branch.kind == crate::BranchKind::Remote
    }));
    assert!(!branches.iter().any(|branch| branch.name == "origin/side"));

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_local_ahead_after_commit_emits_patchable_delta() {
    let remotes_dir = TempDir::new().unwrap();
    let remote_path = remotes_dir.path().join("origin.git");
    init_bare_repo(&remote_path);

    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.git(["remote", "add", "origin", remote_path.to_str().unwrap()]);
    repo.git(["push", "-u", "origin", "main"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let branch_ref_path = baseline.snapshot.identity.git_dir.join("refs/heads/main");
    let baseline_upstream = baseline.snapshot.upstream.as_ref().unwrap();
    assert_eq!(baseline_upstream.ahead, 0);
    assert_eq!(baseline_upstream.behind, 0);

    repo.write("tracked.txt", "ahead\n");
    repo.git(["commit", "-am", "ahead"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(branch_ref_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.head_changed);
    assert!(delta.delta.upstream_changed);
    assert!(delta.delta.branches_changed);
    assert!(!delta.delta.paths.has_changes());
    let upstream = delta.patch.upstream.as_ref().unwrap().as_ref().unwrap();
    assert_eq!(upstream.name, "origin/main");
    assert_eq!(upstream.ahead, 1);
    assert_eq!(upstream.behind, 0);
    let head = delta
        .patch
        .branches
        .as_ref()
        .unwrap()
        .iter()
        .find(|branch| branch.is_head)
        .unwrap();
    assert_eq!(head.upstream_ahead, Some(1));
    assert_eq!(head.upstream_behind, Some(0));

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_local_behind_after_remote_commit_and_fetch_emits_patchable_delta() {
    let remotes_dir = TempDir::new().unwrap();
    let remote_path = remotes_dir.path().join("origin.git");
    init_bare_repo(&remote_path);

    let seed = TestRepo::new();
    seed.write("tracked.txt", "base\n");
    seed.git(["add", "tracked.txt"]);
    seed.git(["commit", "-m", "initial"]);
    seed.git(["remote", "add", "origin", remote_path.to_str().unwrap()]);
    seed.git(["push", "-u", "origin", "main"]);

    let repo = TestRepo::clone_from(&remote_path);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let remote_ref_path = baseline
        .snapshot
        .identity
        .git_dir
        .join("refs/remotes/origin/main");
    let baseline_upstream = baseline.snapshot.upstream.as_ref().unwrap();
    assert_eq!(baseline_upstream.ahead, 0);
    assert_eq!(baseline_upstream.behind, 0);

    let other = TestRepo::clone_from(&remote_path);
    other.write("tracked.txt", "remote\n");
    other.git(["commit", "-am", "remote"]);
    other.git(["push", "origin", "main"]);
    repo.git(["fetch", "origin"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(remote_ref_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(!delta.delta.head_changed);
    assert!(delta.delta.upstream_changed);
    assert!(delta.delta.branches_changed);
    let upstream = delta.patch.upstream.as_ref().unwrap().as_ref().unwrap();
    assert_eq!(upstream.name, "origin/main");
    assert_eq!(upstream.ahead, 0);
    assert_eq!(upstream.behind, 1);
    let head = delta
        .patch
        .branches
        .as_ref()
        .unwrap()
        .iter()
        .find(|branch| branch.is_head)
        .unwrap();
    assert_eq!(head.upstream_ahead, Some(0));
    assert_eq!(head.upstream_behind, Some(1));

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_diverged_after_local_and_remote_commits_emits_patchable_delta() {
    let remotes_dir = TempDir::new().unwrap();
    let remote_path = remotes_dir.path().join("origin.git");
    init_bare_repo(&remote_path);

    let seed = TestRepo::new();
    seed.write("tracked.txt", "base\n");
    seed.git(["add", "tracked.txt"]);
    seed.git(["commit", "-m", "initial"]);
    seed.git(["remote", "add", "origin", remote_path.to_str().unwrap()]);
    seed.git(["push", "-u", "origin", "main"]);

    let repo = TestRepo::clone_from(&remote_path);
    repo.write("local.txt", "local\n");
    repo.git(["add", "local.txt"]);
    repo.git(["commit", "-m", "local"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let remote_ref_path = baseline
        .snapshot
        .identity
        .git_dir
        .join("refs/remotes/origin/main");
    let baseline_upstream = baseline.snapshot.upstream.as_ref().unwrap();
    assert_eq!(baseline_upstream.ahead, 1);
    assert_eq!(baseline_upstream.behind, 0);

    let other = TestRepo::clone_from(&remote_path);
    other.write("remote.txt", "remote\n");
    other.git(["add", "remote.txt"]);
    other.git(["commit", "-m", "remote"]);
    other.git(["push", "origin", "main"]);
    repo.git(["fetch", "origin"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(remote_ref_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(!delta.delta.head_changed);
    assert!(delta.delta.upstream_changed);
    assert!(delta.delta.branches_changed);
    let upstream = delta.patch.upstream.as_ref().unwrap().as_ref().unwrap();
    assert_eq!(upstream.name, "origin/main");
    assert_eq!(upstream.ahead, 1);
    assert_eq!(upstream.behind, 1);
    let head = delta
        .patch
        .branches
        .as_ref()
        .unwrap()
        .iter()
        .find(|branch| branch.is_head)
        .unwrap();
    assert_eq!(head.upstream_ahead, Some(1));
    assert_eq!(head.upstream_behind, Some(1));

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}
