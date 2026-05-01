use super::*;

#[test]
fn includes_remote_and_upstream_summary() {
    let remote = TestRepo::new();
    remote.write("README.md", "hello\n");
    remote.git(["add", "README.md"]);
    remote.git(["commit", "-m", "initial"]);

    let local = TestRepo::clone_from(remote.path());
    local.git(["config", "branch.main.remote", "origin"]);
    local.git(["config", "branch.main.merge", "refs/heads/main"]);

    let snapshot = snapshot_repository(local.path()).unwrap();

    let origin = snapshot
        .remotes
        .iter()
        .find(|remote| remote.name == "origin")
        .unwrap();
    assert_eq!(origin.default_branch.as_deref(), Some("main"));
    assert!(
        origin
            .fetch_refspecs
            .iter()
            .any(|refspec| refspec == "+refs/heads/*:refs/remotes/origin/*")
    );
    assert!(origin.push_refspecs.is_empty());
    assert!(snapshot.upstream.is_some());
    let head = snapshot
        .branches
        .iter()
        .find(|branch| branch.is_head)
        .unwrap();
    assert_eq!(head.upstream.as_deref(), Some("origin/main"));
    assert_eq!(head.upstream_ahead, Some(0));
    assert_eq!(head.upstream_behind, Some(0));
}

#[test]
fn includes_stash_summaries() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    repo.write("tracked.txt", "stashed\n");
    repo.git(["stash", "push", "-m", "save work"]);

    let snapshot = snapshot_repository(repo.path()).unwrap();

    assert_eq!(snapshot.stashes.len(), 1);
    assert_eq!(snapshot.stashes[0].index, 0);
    assert!(snapshot.stashes[0].message.contains("save work"));
    assert!(!snapshot.stashes[0].oid.is_empty());
}

#[test]
fn includes_linked_worktree_summaries() {
    let repo = TestRepo::new();
    repo.write("README.md", "hello\n");
    repo.git(["add", "README.md"]);
    repo.git(["commit", "-m", "initial"]);
    let linked_parent = TempDir::new().unwrap();
    let linked_path = linked_parent.path().join("linked");
    let linked_path = linked_path.to_str().unwrap();
    repo.git(["worktree", "add", "-b", "linked", linked_path]);

    let snapshot = snapshot_repository(repo.path()).unwrap();

    assert_eq!(snapshot.worktrees.len(), 1);
    assert_eq!(snapshot.worktrees[0].name, "linked");
    assert_eq!(snapshot.worktrees[0].path, PathBuf::from(linked_path));
    assert!(!snapshot.worktrees[0].locked);
    assert_eq!(snapshot.worktrees[0].lock_reason, None);
}

#[test]
fn includes_submodule_summaries() {
    let submodule_repo = TestRepo::new();
    submodule_repo.write("README.md", "submodule\n");
    submodule_repo.git(["add", "README.md"]);
    submodule_repo.git(["commit", "-m", "submodule initial"]);
    let submodule_url = submodule_repo.path().to_str().unwrap();

    let repo = TestRepo::new();
    repo.write("README.md", "super\n");
    repo.git(["add", "README.md"]);
    repo.git(["commit", "-m", "super initial"]);
    repo.git_allow_file_protocol(["submodule", "add", submodule_url, "deps/sub"]);
    repo.git(["commit", "-am", "add submodule"]);

    let snapshot = snapshot_repository(repo.path()).unwrap();

    assert_eq!(snapshot.submodules.len(), 1);
    let submodule = &snapshot.submodules[0];
    assert_eq!(submodule.name, "deps/sub");
    assert_eq!(submodule.path, PathBuf::from("deps/sub"));
    assert_eq!(submodule.url.as_deref(), Some(submodule_url));
    assert!(submodule.head_oid.is_some());
    assert!(submodule.index_oid.is_some());
    assert!(submodule.workdir_oid.is_some());
    assert!(submodule.status.in_head);
    assert!(submodule.status.in_index);
    assert!(submodule.status.in_config);
    assert!(submodule.status.in_workdir);
}
