use super::*;

#[test]
fn targeted_path_refresh_updates_paths_without_rebuilding_unrelated_domains() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    let baseline = snapshot_repository(repo.path()).unwrap();

    repo.write("tracked.txt", "changed\n");
    let refresh = refresh_repository_with_plan(
        repo.path(),
        Some(&baseline),
        &RefreshPlan::domains([RefreshDomain::Paths]),
        SnapshotOptions::default(),
    )
    .unwrap();

    assert_eq!(refresh.plan, RefreshPlan::domains([RefreshDomain::Paths]));
    assert_eq!(refresh.snapshot.paths.unstaged, vec!["tracked.txt"]);
    assert_eq!(refresh.snapshot.head, baseline.head);
    assert_eq!(refresh.snapshot.branches, baseline.branches);
    assert_eq!(refresh.snapshot.remotes, baseline.remotes);
}

#[test]
fn targeted_ref_refresh_updates_head_and_branch_state() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "main\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.git(["switch", "-c", "side"]);
    repo.write("tracked.txt", "side\n");
    repo.git(["commit", "-am", "side"]);
    repo.git(["switch", "main"]);
    let baseline = snapshot_repository(repo.path()).unwrap();

    repo.git(["switch", "side"]);
    let refresh = refresh_repository_with_plan(
        repo.path(),
        Some(&baseline),
        &RefreshPlan::domains([
            RefreshDomain::Head,
            RefreshDomain::Upstream,
            RefreshDomain::Branches,
            RefreshDomain::Paths,
        ]),
        SnapshotOptions::default(),
    )
    .unwrap();

    assert_eq!(refresh.snapshot.head.branch.as_deref(), Some("side"));
    assert_ne!(refresh.snapshot.head, baseline.head);
    assert_ne!(refresh.snapshot.branches, baseline.branches);
    assert!(refresh.snapshot.paths.unstaged.is_empty());
}
