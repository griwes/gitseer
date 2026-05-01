use super::*;

#[test]
fn command_shape_git_tag_lightweight_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let tag_ref_path = baseline.snapshot.identity.git_dir.join("refs/tags/v1.0.0");

    repo.git(["tag", "v1.0.0"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(tag_ref_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.tags_changed);
    let tags = delta.patch.tags.as_ref().unwrap();
    let tag = tags.iter().find(|tag| tag.name == "v1.0.0").unwrap();
    assert_eq!(tag.kind, crate::TagKind::Lightweight);

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_tag_annotated_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let tag_ref_path = baseline.snapshot.identity.git_dir.join("refs/tags/v1.0.0");

    repo.git(["tag", "-a", "v1.0.0", "-m", "release"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(tag_ref_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.tags_changed);
    let tags = delta.patch.tags.as_ref().unwrap();
    let tag = tags.iter().find(|tag| tag.name == "v1.0.0").unwrap();
    assert_eq!(tag.kind, crate::TagKind::Annotated);
    assert_eq!(tag.message.as_deref(), Some("release\n"));
    assert_eq!(tag.tagger_name.as_deref(), Some("Tester"));
    assert_eq!(tag.tagger_email.as_deref(), Some("tester@example.com"));

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_tag_delete_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.git(["tag", "v1.0.0"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let tag_ref_path = baseline.snapshot.identity.git_dir.join("refs/tags/v1.0.0");
    assert!(
        baseline
            .snapshot
            .tags
            .iter()
            .any(|tag| tag.name == "v1.0.0")
    );

    repo.git(["tag", "-d", "v1.0.0"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(tag_ref_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(delta.delta.tags_changed);
    assert!(delta.patch.tags.as_ref().unwrap().is_empty());

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}

#[test]
fn command_shape_git_pack_refs_all_emits_no_semantic_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.git(["branch", "side"]);
    repo.git(["tag", "v1.0.0"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let packed_refs_path = baseline.snapshot.identity.git_dir.join("packed-refs");

    repo.git(["pack-refs", "--all"]);
    let plan = update_from_watch_event_with_no_delta(&mut state, event_for(packed_refs_path));

    assert_incremental_refresh_plan(&plan);
    let refresh = refresh_repository_with_plan(
        state.repo(),
        Some(&baseline.snapshot),
        &plan,
        SnapshotOptions::default(),
    )
    .unwrap();
    assert_eq!(refresh.plan, plan);
    let messages = snapshot_update_messages(&mut state, refresh.snapshot);
    assert!(messages.is_empty());
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(fresh, baseline.snapshot);
}

#[test]
fn command_shape_update_branch_with_packed_previous_ref_emits_patchable_delta() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.git(["branch", "side"]);
    repo.write("tracked.txt", "second\n");
    repo.git(["commit", "-am", "second"]);
    let main_oid = repo.git_stdout(["rev-parse", "main"]);
    repo.git(["pack-refs", "--all"]);
    let mut state = ProcessState::new(repo.path());
    let baseline = subscribe_for_deltas(&mut state);
    let side_ref_path = baseline.snapshot.identity.git_dir.join("refs/heads/side");
    let baseline_side = baseline
        .snapshot
        .branches
        .iter()
        .find(|branch| branch.name == "side")
        .unwrap();
    assert_ne!(baseline_side.oid.as_deref(), Some(main_oid.as_str()));

    repo.git(["branch", "-f", "side", "main"]);
    let (plan, delta) = update_from_watch_event(&mut state, event_for(side_ref_path));

    assert_incremental_refresh_plan(&plan);
    assert_eq!(delta.previous_version, baseline.version);
    assert_eq!(delta.version, baseline.version + 1);
    assert!(!delta.delta.head_changed);
    assert!(delta.delta.branches_changed);
    let side = delta
        .patch
        .branches
        .as_ref()
        .unwrap()
        .iter()
        .find(|branch| branch.name == "side")
        .unwrap();
    assert_eq!(side.oid.as_deref(), Some(main_oid.as_str()));

    let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
    let fresh = snapshot_repository(repo.path()).unwrap();
    assert_eq!(patched, fresh);
}
