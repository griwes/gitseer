use std::fs;
use std::path::Path;
use std::process::Command;

use crate::RefreshDomain;
use notify::RecursiveMode;
use notify::event::{AccessKind, AccessMode, EventKind, Flag};
use tempfile::TempDir;

use super::roots::watch_mode_for_root;
use super::*;

#[test]
fn watch_roots_include_worktree_and_git_metadata() {
    let repo = TestRepo::new();
    repo.write("README.md", "hello\n");
    repo.git(["add", "README.md"]);
    repo.git(["commit", "-m", "initial"]);

    let snapshot = snapshot_repository(repo.path()).unwrap();
    let roots = watch_roots_for_snapshot(&snapshot);

    assert!(roots.iter().any(|root| root == repo.path()));
    assert!(roots.iter().any(|root| root.ends_with(".git")));
}

#[test]
fn watch_roots_include_configured_core_excludesfile() {
    let excludes_dir = TempDir::new().unwrap();
    let excludes_path = excludes_dir.path().join("global-ignore");
    fs::write(&excludes_path, "").unwrap();
    let repo = TestRepo::new();
    repo.write("README.md", "hello\n");
    repo.git(["add", "README.md"]);
    repo.git(["commit", "-m", "initial"]);
    repo.git([
        "config",
        "core.excludesfile",
        excludes_path.to_str().unwrap(),
    ]);

    let snapshot = snapshot_repository(repo.path()).unwrap();
    let roots = watch_roots_for_snapshot(&snapshot);

    assert!(roots.iter().any(|root| root == &excludes_path));
    assert_eq!(
        watch_mode_for_root(&snapshot, &excludes_path),
        RecursiveMode::NonRecursive
    );
}

#[test]
fn refresh_policy_skips_plain_access_events() {
    let event = Ok(Event::new(EventKind::Access(AccessKind::Close(
        AccessMode::Read,
    ))));

    assert!(!should_refresh_for_event(&event));
}

#[test]
fn refresh_policy_treats_write_access_as_refresh_worthy() {
    let event = Ok(Event::new(EventKind::Access(AccessKind::Close(
        AccessMode::Write,
    ))));

    assert!(should_refresh_for_event(&event));
}

#[test]
fn refresh_policy_treats_rescan_and_errors_as_full_refreshes() {
    let rescan_event = Ok(Event::new(EventKind::Any).set_flag(Flag::Rescan));
    let error_event = Err(notify::Error::generic("overflow"));

    assert!(should_refresh_for_event(&rescan_event));
    assert!(should_refresh_for_event(&error_event));
}

#[test]
fn worktree_watch_uses_nonrecursive_mode() {
    let repo = TestRepo::new();
    repo.write("README.md", "hello\n");
    repo.git(["add", "README.md"]);
    repo.git(["commit", "-m", "initial"]);
    let snapshot = snapshot_repository(repo.path()).unwrap();

    assert_eq!(
        watch_mode_for_root(&snapshot, repo.path()),
        RecursiveMode::NonRecursive
    );
}

#[test]
fn watch_roots_skip_ignored_worktree_directories() {
    let repo = TestRepo::new();
    repo.write(".gitignore", "build/\n");
    repo.write("src/main.rs", "fn main() {}\n");
    repo.git(["add", ".gitignore", "src/main.rs"]);
    repo.git(["commit", "-m", "initial"]);
    fs::create_dir_all(repo.path().join("build/generated/deep")).unwrap();
    fs::create_dir_all(repo.path().join("src/nested")).unwrap();

    let snapshot = snapshot_repository(repo.path()).unwrap();
    let roots = watch_roots_for_snapshot(&snapshot);

    assert!(roots.iter().any(|root| root == repo.path()));
    assert!(roots.iter().any(|root| root.ends_with("src")));
    assert!(roots.iter().any(|root| root.ends_with("src/nested")));
    assert!(!roots.iter().any(|root| root.ends_with("build")));
    assert!(!roots.iter().any(|root| root.ends_with("build/generated")));
    assert!(
        !roots
            .iter()
            .any(|root| root.ends_with("build/generated/deep"))
    );
}

#[test]
fn refresh_plan_filters_ignored_worktree_paths() {
    let repo = TestRepo::new();
    repo.write(".gitignore", "build/\n");
    repo.git(["add", ".gitignore"]);
    repo.git(["commit", "-m", "ignore build"]);
    fs::create_dir_all(repo.path().join("build")).unwrap();
    repo.write("build/output.o", "object\n");
    let snapshot = snapshot_repository(repo.path()).unwrap();
    let event = event_for(repo.path().join("build/output.o"));

    let plan = refresh_plan_for_event(
        &Ok(event),
        repo.path(),
        snapshot.identity.worktree_root.as_deref(),
        &snapshot.identity.git_dir,
        &snapshot.identity.common_dir,
    );

    assert_eq!(plan, RefreshPlan::None);
}

#[test]
fn refresh_plan_tracks_ignore_rule_changes() {
    let repo = TestRepo::new();
    repo.write(".gitignore", "build/\n");
    repo.git(["add", ".gitignore"]);
    repo.git(["commit", "-m", "ignore build"]);
    let snapshot = snapshot_repository(repo.path()).unwrap();
    let event = event_for(repo.path().join(".gitignore"));

    let plan = refresh_plan_for_event(
        &Ok(event),
        repo.path(),
        snapshot.identity.worktree_root.as_deref(),
        &snapshot.identity.git_dir,
        &snapshot.identity.common_dir,
    );

    assert_eq!(plan, RefreshPlan::domains([RefreshDomain::Paths]));
}

#[test]
fn refresh_plan_maps_git_metadata_to_domains() {
    let repo = TestRepo::new();
    repo.write("README.md", "hello\n");
    repo.git(["add", "README.md"]);
    repo.git(["commit", "-m", "initial"]);
    let snapshot = snapshot_repository(repo.path()).unwrap();
    let event = event_for(snapshot.identity.git_dir.join("HEAD"));

    let plan = refresh_plan_for_event(
        &Ok(event),
        repo.path(),
        snapshot.identity.worktree_root.as_deref(),
        &snapshot.identity.git_dir,
        &snapshot.identity.common_dir,
    );

    assert_eq!(
        plan,
        RefreshPlan::domains([
            RefreshDomain::Head,
            RefreshDomain::Upstream,
            RefreshDomain::Branches,
            RefreshDomain::Paths,
        ])
    );
}

#[tokio::test]
async fn debounce_drains_pending_bursts() {
    let (tx, mut rx) = mpsc::channel::<notify::Result<Event>>(8);
    for _ in 0..5 {
        tx.send(Ok(Event::new(EventKind::Any))).await.unwrap();
    }

    let mut drained = 0;
    while rx.try_recv().is_ok() {
        drained += 1;
    }

    assert_eq!(drained, 5);
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn repository_watcher_receives_worktree_events() {
    let repo = TestRepo::new();
    repo.write("README.md", "hello\n");
    repo.git(["add", "README.md"]);
    repo.git(["commit", "-m", "initial"]);

    let mut watcher = RepositoryWatcher::new(repo.path()).unwrap();
    repo.write("README.md", "changed\n");

    let event = tokio::time::timeout(Duration::from_secs(5), watcher.next_event())
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    assert!(!event.paths.is_empty());
}

#[tokio::test]
async fn repository_watcher_receives_index_events() {
    let repo = TestRepo::new();
    repo.write("README.md", "hello\n");
    repo.git(["add", "README.md"]);
    repo.git(["commit", "-m", "initial"]);

    let mut watcher = RepositoryWatcher::new(repo.path()).unwrap();
    repo.write("staged.txt", "staged\n");
    repo.git(["add", "staged.txt"]);

    let event = tokio::time::timeout(Duration::from_secs(5), watcher.next_event())
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    assert!(!event.paths.is_empty());
}

#[tokio::test]
async fn repository_watcher_receives_branch_switch_events() {
    let repo = TestRepo::new();
    repo.write("README.md", "main\n");
    repo.git(["add", "README.md"]);
    repo.git(["commit", "-m", "initial"]);
    repo.git(["checkout", "-b", "side"]);
    repo.write("README.md", "side\n");
    repo.git(["commit", "-am", "side"]);
    repo.git(["checkout", "main"]);

    let mut watcher = RepositoryWatcher::new(repo.path()).unwrap();
    repo.git(["checkout", "side"]);

    let event = tokio::time::timeout(Duration::from_secs(5), watcher.next_event())
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    assert!(!event.paths.is_empty());
}

#[tokio::test]
async fn repository_watcher_does_not_watch_existing_ignored_directory_contents() {
    let repo = TestRepo::new();
    repo.write(".gitignore", "build/\n");
    repo.git(["add", ".gitignore"]);
    repo.git(["commit", "-m", "ignore build"]);
    fs::create_dir_all(repo.path().join("build")).unwrap();

    let mut watcher = RepositoryWatcher::new(repo.path()).unwrap();
    repo.write("build/output.o", "object\n");

    assert_no_event_path_under(&mut watcher, "build").await;
}

#[tokio::test]
async fn repository_watcher_adds_watches_for_new_worktree_directories() {
    let repo = TestRepo::new();
    repo.write("README.md", "hello\n");
    repo.git(["add", "README.md"]);
    repo.git(["commit", "-m", "initial"]);

    let mut watcher = RepositoryWatcher::new(repo.path()).unwrap();
    fs::create_dir_all(repo.path().join("src/nested")).unwrap();
    wait_for_watched_root(&mut watcher, "src/nested").await;

    repo.write("src/nested/main.rs", "fn main() {}\n");

    wait_for_event_path(&mut watcher, "main.rs").await;
}

#[tokio::test]
async fn repository_watcher_does_not_add_watches_for_new_ignored_directories() {
    let repo = TestRepo::new();
    repo.write(".gitignore", "build/\n");
    repo.git(["add", ".gitignore"]);
    repo.git(["commit", "-m", "ignore build"]);

    let mut watcher = RepositoryWatcher::new(repo.path()).unwrap();
    fs::create_dir_all(repo.path().join("build")).unwrap();
    wait_for_event_path(&mut watcher, "build").await;

    assert!(
        !watcher
            .watched_roots
            .iter()
            .any(|path| path.ends_with("build")),
        "new ignored directories should not be added to the watch set"
    );
}

#[tokio::test]
async fn repository_watcher_reconciles_watches_after_unignore() {
    let repo = TestRepo::new();
    repo.write(".gitignore", "build/\n");
    repo.git(["add", ".gitignore"]);
    repo.git(["commit", "-m", "ignore build"]);
    fs::create_dir_all(repo.path().join("build")).unwrap();

    let mut watcher = RepositoryWatcher::new(repo.path()).unwrap();
    assert!(
        !watcher
            .watched_roots
            .iter()
            .any(|path| path.ends_with("build"))
    );

    repo.write(".gitignore", "");
    wait_for_watched_root(&mut watcher, "build").await;

    repo.write("build/output.log", "artifact\n");
    wait_for_event_path(&mut watcher, "output.log").await;
}

#[tokio::test]
async fn repository_watcher_reconciles_watches_after_git_info_exclude_unignore() {
    let repo = TestRepo::new();
    repo.write("tracked.txt", "base\n");
    repo.git(["add", "tracked.txt"]);
    repo.git(["commit", "-m", "initial"]);
    fs::write(repo.path().join(".git/info/exclude"), "build/\n").unwrap();
    fs::create_dir_all(repo.path().join("build")).unwrap();

    let mut watcher = RepositoryWatcher::new(repo.path()).unwrap();
    assert!(
        !watcher
            .watched_roots
            .iter()
            .any(|path| path.ends_with("build"))
    );

    fs::write(repo.path().join(".git/info/exclude"), "").unwrap();
    wait_for_watched_root(&mut watcher, "build").await;
}

#[tokio::test]
async fn repository_watcher_reconciles_watches_after_rescan_event() {
    let repo = TestRepo::new();
    repo.write(".gitignore", "build/\n");
    repo.git(["add", ".gitignore"]);
    repo.git(["commit", "-m", "ignore build"]);
    fs::create_dir_all(repo.path().join("build")).unwrap();

    let mut watcher = RepositoryWatcher::new(repo.path()).unwrap();
    assert!(
        !watcher
            .watched_roots
            .iter()
            .any(|path| path.ends_with("build"))
    );

    repo.write(".gitignore", "");
    let event = Event::new(EventKind::Any).set_flag(Flag::Rescan);
    watcher.update_watches_for_event(&Ok(event)).await;

    assert!(
        watcher
            .watched_roots
            .iter()
            .any(|path| path.ends_with("build")),
        "rescan events should reconcile the watch set"
    );
}

#[tokio::test]
async fn repository_watcher_rewatches_recreated_directory() {
    let repo = TestRepo::new();
    repo.write("README.md", "hello\n");
    repo.git(["add", "README.md"]);
    repo.git(["commit", "-m", "initial"]);

    let mut watcher = RepositoryWatcher::new(repo.path()).unwrap();
    fs::create_dir_all(repo.path().join("src")).unwrap();
    wait_for_watched_root(&mut watcher, "src").await;

    fs::remove_dir_all(repo.path().join("src")).unwrap();
    watcher.prune_stale_worktree_watch_roots(repo.path());
    assert!(
        !watcher
            .watched_roots
            .iter()
            .any(|path| path.ends_with("src")),
        "stale removed directories should be pruned before rewatching"
    );

    fs::create_dir_all(repo.path().join("src")).unwrap();
    wait_for_watched_root(&mut watcher, "src").await;

    repo.write("src/main.rs", "fn main() {}\n");
    wait_for_event_path(&mut watcher, "main.rs").await;
}

async fn wait_for_watched_root(watcher: &mut RepositoryWatcher, suffix: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut seen = 0;
    while !watcher
        .watched_roots
        .iter()
        .any(|path| path.ends_with(suffix))
    {
        let Some(remaining) = deadline.checked_duration_since(tokio::time::Instant::now()) else {
            panic!("timed out waiting for watched root ending in {suffix}; saw {seen} events");
        };
        tokio::time::timeout(remaining, watcher.next_event())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        seen += 1;
    }
}

async fn wait_for_event_path(watcher: &mut RepositoryWatcher, suffix: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut seen = 0;
    loop {
        let Some(remaining) = deadline.checked_duration_since(tokio::time::Instant::now()) else {
            panic!("timed out waiting for event path ending in {suffix}; saw {seen} other events");
        };
        let event = tokio::time::timeout(remaining, watcher.next_event())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        seen += 1;
        if event.paths.iter().any(|path| path.ends_with(suffix)) {
            return;
        }
    }
}

async fn assert_no_event_path_under(watcher: &mut RepositoryWatcher, path: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(300);
    loop {
        let Some(remaining) = deadline.checked_duration_since(tokio::time::Instant::now()) else {
            return;
        };
        let Ok(Some(Ok(event))) = tokio::time::timeout(remaining, watcher.next_event()).await
        else {
            return;
        };
        assert!(
            !event
                .paths
                .iter()
                .any(|event_path| event_path.ends_with(path)),
            "ignored path should not be watched: {event:?}"
        );
    }
}

fn event_for(path: impl Into<PathBuf>) -> Event {
    let mut event = Event::new(EventKind::Any);
    event.paths.push(path.into());
    event
}

struct TestRepo {
    temp: TempDir,
}

impl TestRepo {
    fn new() -> Self {
        let temp = TempDir::new().unwrap();
        let repo = Self { temp };
        repo.git(["init", "--initial-branch=main"]);
        repo.git(["config", "user.email", "tester@example.com"]);
        repo.git(["config", "user.name", "Tester"]);
        repo.git(["config", "commit.gpgsign", "false"]);
        repo
    }

    fn path(&self) -> &Path {
        self.temp.path()
    }

    fn write(&self, relative: &str, contents: &str) {
        let path = self.path().join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    fn git<const N: usize>(&self, args: [&str; N]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(self.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git command failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
