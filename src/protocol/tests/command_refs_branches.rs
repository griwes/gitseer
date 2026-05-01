use super::*;

#[test]
fn command_shape_git_branch_name_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let branch_ref_path = baseline.snapshot.identity.git_dir.join("refs/heads/side");

    repo.git(["branch", "side"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(branch_ref_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(!delta.delta.head_changed);
    assert!(delta.delta.branches_changed);
    assert!(
        delta
            .patch
            .branches
            .as_ref()
            .unwrap()
            .iter()
            .any(|branch| branch.name == "side")
    );

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_switch_create_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let head_path = baseline.snapshot.identity.git_dir.join("HEAD");

    repo.git(["switch", "-c", "side"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(head_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.head_changed);
    assert!(delta.delta.branches_changed);
    assert_eq!(
        delta.patch.head.as_ref().unwrap().branch.as_deref(),
        Some("side")
    );
    assert!(
        delta
            .patch
            .branches
            .as_ref()
            .unwrap()
            .iter()
            .any(|branch| branch.name == "side" && branch.is_head)
    );

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_switch_name_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.git(["branch", "side"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let head_path = baseline.snapshot.identity.git_dir.join("HEAD");

    repo.git(["switch", "side"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(head_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.head_changed);
    assert!(delta.delta.branches_changed);
    assert_eq!(
        delta.patch.head.as_ref().unwrap().branch.as_deref(),
        Some("side")
    );
    assert!(
        delta
            .patch
            .branches
            .as_ref()
            .unwrap()
            .iter()
            .any(|branch| branch.name == "side" && branch.is_head)
    );

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_checkout_create_branch_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let head_path = baseline.snapshot.identity.git_dir.join("HEAD");

    repo.git(["checkout", "-b", "side"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(head_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.head_changed);
    assert!(delta.delta.branches_changed);
    assert_eq!(
        delta.patch.head.as_ref().unwrap().branch.as_deref(),
        Some("side")
    );
    assert!(
        delta
            .patch
            .branches
            .as_ref()
            .unwrap()
            .iter()
            .any(|branch| branch.name == "side" && branch.is_head)
    );

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_checkout_commit_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    let first_oid = repo.git_stdout(["rev-parse", "HEAD"]);
    repo.write("tracked.txt", "second\n");
    repo.git(["commit", "-am", "second"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let head_path = baseline.snapshot.identity.git_dir.join("HEAD");

    repo.git(["checkout", first_oid.as_str()]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(head_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.head_changed);
    assert!(delta.delta.branches_changed);
    let head = delta.patch.head.as_ref().unwrap();
    assert_eq!(head.kind, HeadKind::Detached);
    assert_eq!(head.oid.as_deref(), Some(first_oid.as_str()));

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_switch_previous_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.git(["switch", "-c", "side"]);
    repo.git(["switch", "main"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let head_path = baseline.snapshot.identity.git_dir.join("HEAD");

    repo.git(["switch", "-"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(head_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.head_changed);
    assert!(delta.delta.branches_changed);
    assert_eq!(
        delta.patch.head.as_ref().unwrap().branch.as_deref(),
        Some("side")
    );
    assert!(
        delta
            .patch
            .branches
            .as_ref()
            .unwrap()
            .iter()
            .any(|branch| branch.name == "side" && branch.is_head)
    );

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_branch_rename_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.git(["branch", "side"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let branch_ref_path = baseline
        .snapshot
        .identity
        .git_dir
        .join("refs/heads/renamed");

    repo.git(["branch", "-m", "side", "renamed"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(branch_ref_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(!delta.delta.head_changed);
    assert!(delta.delta.branches_changed);
    let branches = delta.patch.branches.as_ref().unwrap();
    assert!(branches.iter().any(|branch| branch.name == "renamed"));
    assert!(!branches.iter().any(|branch| branch.name == "side"));

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_branch_delete_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.git(["branch", "side"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let branch_ref_path = baseline.snapshot.identity.git_dir.join("refs/heads/side");

    repo.git(["branch", "-d", "side"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(branch_ref_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(!delta.delta.head_changed);
    assert!(delta.delta.branches_changed);
    assert!(
        !delta
            .patch
            .branches
            .as_ref()
            .unwrap()
            .iter()
            .any(|branch| branch.name == "side")
    );

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_reset_soft_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    let first_oid = repo.git_stdout(["rev-parse", "HEAD"]);
    repo.write("tracked.txt", "second\n");
    repo.git(["commit", "-am", "second"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let branch_ref_path = baseline.snapshot.identity.git_dir.join("refs/heads/main");

    repo.git(["reset", "--soft", first_oid.as_str()]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(branch_ref_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.head_changed);
    assert!(delta.delta.branches_changed);
    assert_eq!(
        delta.patch.head.as_ref().unwrap().oid.as_deref(),
        Some(first_oid.as_str())
    );
    let paths = delta.patch.paths.as_ref().unwrap();
    assert_eq!(paths.staged, vec!["tracked.txt"]);
    assert!(paths.unstaged.is_empty());

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_reset_mixed_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    let first_oid = repo.git_stdout(["rev-parse", "HEAD"]);
    repo.write("tracked.txt", "second\n");
    repo.git(["commit", "-am", "second"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let branch_ref_path = baseline.snapshot.identity.git_dir.join("refs/heads/main");

    repo.git(["reset", "--mixed", first_oid.as_str()]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(branch_ref_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.head_changed);
    assert!(delta.delta.branches_changed);
    assert_eq!(
        delta.patch.head.as_ref().unwrap().oid.as_deref(),
        Some(first_oid.as_str())
    );
    let paths = delta.patch.paths.as_ref().unwrap();
    assert!(paths.staged.is_empty());
    assert_eq!(paths.unstaged, vec!["tracked.txt"]);

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_reset_hard_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    let first_oid = repo.git_stdout(["rev-parse", "HEAD"]);
    repo.write("tracked.txt", "second\n");
    repo.git(["commit", "-am", "second"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let branch_ref_path = baseline.snapshot.identity.git_dir.join("refs/heads/main");

    repo.git(["reset", "--hard", first_oid.as_str()]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(branch_ref_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.head_changed);
    assert!(delta.delta.branches_changed);
    assert!(!delta.delta.paths.has_changes());
    assert_eq!(
        delta.patch.head.as_ref().unwrap().oid.as_deref(),
        Some(first_oid.as_str())
    );
    assert!(delta.patch.paths.is_none());

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_commit_amend_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.write("tracked.txt", "amended\n");
    repo.git(["add", "tracked.txt"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let branch_ref_path = baseline.snapshot.identity.git_dir.join("refs/heads/main");
    let previous_oid = baseline.snapshot.head.oid.clone();

    repo.git(["commit", "--amend", "--no-edit"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(branch_ref_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.head_changed);
    assert!(delta.delta.branches_changed);
    assert_eq!(delta.delta.paths.staged.removed, vec!["tracked.txt"]);
    let head = delta.patch.head.as_ref().unwrap();
    assert_ne!(head.oid, previous_oid);
    let paths = delta.patch.paths.as_ref().unwrap();
    assert!(paths.staged.is_empty());
    assert!(paths.unstaged.is_empty());

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}
