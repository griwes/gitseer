use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use thiserror::Error;
use tokio::sync::mpsc;

use crate::{RefreshDomain, RefreshPlan, RepositorySnapshot, SnapshotError, snapshot_repository};

#[derive(Debug, Error)]
pub enum WatchError {
    #[error(transparent)]
    Snapshot(#[from] SnapshotError),
    #[error(transparent)]
    Notify(#[from] notify::Error),
}

pub struct RepositoryWatcher {
    _watcher: RecommendedWatcher,
    rx: mpsc::Receiver<notify::Result<Event>>,
    watched_roots: Vec<PathBuf>,
    repo_path: PathBuf,
    worktree_root: Option<PathBuf>,
    git_dir: PathBuf,
    common_dir: PathBuf,
}

impl RepositoryWatcher {
    pub fn new(repo_path: impl AsRef<Path>) -> Result<Self, WatchError> {
        let repo_path = repo_path.as_ref().to_path_buf();
        let snapshot = snapshot_repository(&repo_path)?;
        let watched_roots = watch_roots_for_snapshot(&snapshot);
        let (tx, rx) = mpsc::channel(256);
        let mut watcher = notify::recommended_watcher(move |event| {
            let _ = tx.blocking_send(event);
        })?;

        for root in &watched_roots {
            watcher.watch(root, watch_mode_for_root(&snapshot, root))?;
        }

        Ok(Self {
            _watcher: watcher,
            rx,
            watched_roots,
            repo_path,
            worktree_root: snapshot.identity.worktree_root,
            git_dir: snapshot.identity.git_dir,
            common_dir: snapshot.identity.common_dir,
        })
    }

    pub fn watched_roots(&self) -> &[PathBuf] {
        &self.watched_roots
    }

    pub async fn next_event(&mut self) -> Option<notify::Result<Event>> {
        self.rx.recv().await
    }

    pub async fn debounce_plan(&mut self, initial: RefreshPlan, duration: Duration) -> RefreshPlan {
        tokio::time::sleep(duration).await;
        drain_pending_plan(self, initial)
    }

    pub fn refresh_plan_for_event(&self, event: &notify::Result<Event>) -> RefreshPlan {
        refresh_plan_for_event(
            event,
            &self.repo_path,
            self.worktree_root.as_deref(),
            &self.git_dir,
            &self.common_dir,
        )
    }
}

pub fn should_refresh_for_event(event: &notify::Result<Event>) -> bool {
    match event {
        Ok(event) => event.need_rescan() || is_mutating_event(event),
        Err(_) => true,
    }
}

pub fn refresh_plan_for_event(
    event: &notify::Result<Event>,
    repo_path: &Path,
    worktree_root: Option<&Path>,
    git_dir: &Path,
    common_dir: &Path,
) -> RefreshPlan {
    match event {
        Ok(event) if event.need_rescan() => RefreshPlan::Full,
        Ok(event) if !is_mutating_event(event) => RefreshPlan::None,
        Ok(event) => {
            let mut plan = RefreshPlan::None;
            for path in &event.paths {
                plan = plan.combine(refresh_plan_for_path(
                    repo_path,
                    worktree_root,
                    git_dir,
                    common_dir,
                    path,
                ));
            }
            if event.paths.is_empty() {
                RefreshPlan::Full
            } else {
                plan
            }
        }
        Err(_) => RefreshPlan::Full,
    }
}

fn is_mutating_event(event: &Event) -> bool {
    !matches!(
        event.kind,
        notify::event::EventKind::Access(
            notify::event::AccessKind::Read
                | notify::event::AccessKind::Open(notify::event::AccessMode::Read)
                | notify::event::AccessKind::Close(notify::event::AccessMode::Read)
        )
    )
}

fn refresh_plan_for_path(
    repo_path: &Path,
    worktree_root: Option<&Path>,
    git_dir: &Path,
    common_dir: &Path,
    path: &Path,
) -> RefreshPlan {
    if path.starts_with(git_dir) || path.starts_with(common_dir) {
        return refresh_plan_for_git_path(repo_path, git_dir, common_dir, path);
    }
    if is_configured_excludes_file(repo_path, path) {
        return RefreshPlan::domains([RefreshDomain::Paths]);
    }

    let Some(worktree_root) = worktree_root else {
        return RefreshPlan::Full;
    };

    if !path.starts_with(worktree_root) {
        return RefreshPlan::Full;
    }

    let Ok(relative) = path.strip_prefix(worktree_root) else {
        return RefreshPlan::Full;
    };

    if is_ignore_rule_path(relative) {
        return RefreshPlan::domains([RefreshDomain::Paths]);
    }

    if relative == Path::new(".git") {
        return RefreshPlan::domains([RefreshDomain::Identity, RefreshDomain::Worktrees]);
    }

    if relative == Path::new(".gitmodules") || path_has_component(relative, ".git") {
        return RefreshPlan::domains([RefreshDomain::Paths, RefreshDomain::Submodules]);
    }

    if path_is_inside_submodule(repo_path, relative) {
        return RefreshPlan::domains([RefreshDomain::Paths, RefreshDomain::Submodules]);
    }

    match git2::Repository::discover(repo_path).and_then(|repo| repo.status_should_ignore(relative))
    {
        Ok(true) => RefreshPlan::None,
        Ok(false) => RefreshPlan::domains([RefreshDomain::Paths]),
        Err(_) => RefreshPlan::domains([RefreshDomain::Paths]),
    }
}

fn refresh_plan_for_git_path(
    repo_path: &Path,
    git_dir: &Path,
    common_dir: &Path,
    path: &Path,
) -> RefreshPlan {
    let relative = path
        .strip_prefix(git_dir)
        .or_else(|_| path.strip_prefix(common_dir))
        .unwrap_or(path);
    let text = relative.to_string_lossy();

    if text == "index"
        || text.starts_with("index.")
        || text == "info/exclude"
        || text == "info/sparse-checkout"
    {
        return RefreshPlan::domains([RefreshDomain::Paths]);
    }
    if text == "HEAD" || text == "ORIG_HEAD" {
        return RefreshPlan::domains([
            RefreshDomain::Head,
            RefreshDomain::Upstream,
            RefreshDomain::Branches,
            RefreshDomain::Paths,
        ]);
    }
    if text == "shallow" {
        return RefreshPlan::domains([
            RefreshDomain::Identity,
            RefreshDomain::Head,
            RefreshDomain::Upstream,
            RefreshDomain::Branches,
        ]);
    }
    if text.starts_with("refs/heads/") {
        let mut domains = vec![
            RefreshDomain::Head,
            RefreshDomain::Upstream,
            RefreshDomain::Branches,
        ];
        if is_current_branch_ref(repo_path, &text) {
            domains.push(RefreshDomain::Paths);
        }
        return RefreshPlan::domains(domains);
    }
    if text.starts_with("refs/remotes/") || text == "FETCH_HEAD" {
        return RefreshPlan::domains([
            RefreshDomain::Upstream,
            RefreshDomain::Branches,
            RefreshDomain::Remotes,
        ]);
    }
    if text.starts_with("refs/tags/") {
        return RefreshPlan::domains([RefreshDomain::Tags]);
    }
    if text == "packed-refs" {
        return RefreshPlan::domains([
            RefreshDomain::Head,
            RefreshDomain::Upstream,
            RefreshDomain::Branches,
            RefreshDomain::Tags,
        ]);
    }
    if text == "config" {
        return RefreshPlan::domains([
            RefreshDomain::Remotes,
            RefreshDomain::Upstream,
            RefreshDomain::Branches,
            RefreshDomain::Paths,
        ]);
    }
    if text == "logs/refs/stash" || text == "refs/stash" {
        return RefreshPlan::domains([
            RefreshDomain::Head,
            RefreshDomain::Upstream,
            RefreshDomain::Branches,
            RefreshDomain::Paths,
            RefreshDomain::Stashes,
        ]);
    }
    if text.starts_with("worktrees/") || text == "commondir" || text == "gitdir" {
        return RefreshPlan::domains([
            RefreshDomain::Identity,
            RefreshDomain::Upstream,
            RefreshDomain::Branches,
            RefreshDomain::Worktrees,
        ]);
    }
    if is_operation_path(&text) {
        return RefreshPlan::domains([
            RefreshDomain::Operation,
            RefreshDomain::Head,
            RefreshDomain::Upstream,
            RefreshDomain::Branches,
            RefreshDomain::Paths,
        ]);
    }

    RefreshPlan::Full
}

fn drain_pending_plan(watcher: &mut RepositoryWatcher, initial: RefreshPlan) -> RefreshPlan {
    let mut plan = initial;
    while let Ok(event) = watcher.rx.try_recv() {
        plan = plan.combine(watcher.refresh_plan_for_event(&event));
    }
    plan
}

fn is_ignore_rule_path(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == ".gitignore")
}

fn path_has_component(path: &Path, component: &str) -> bool {
    path.components()
        .any(|part| part.as_os_str().to_string_lossy() == component)
}

fn path_is_inside_submodule(repo_path: &Path, relative: &Path) -> bool {
    let Ok(repo) = git2::Repository::discover(repo_path) else {
        return false;
    };
    let Ok(submodules) = repo.submodules() else {
        return false;
    };

    submodules.iter().any(|submodule| {
        let submodule_path = submodule.path();
        relative == submodule_path || relative.starts_with(submodule_path)
    })
}

fn is_operation_path(path: &str) -> bool {
    matches!(
        path,
        "MERGE_HEAD"
            | "REBASE_HEAD"
            | "CHERRY_PICK_HEAD"
            | "REVERT_HEAD"
            | "BISECT_LOG"
            | "BISECT_START"
            | "MERGE_MSG"
    ) || path.starts_with("sequencer/")
        || path.starts_with("rebase-merge/")
        || path.starts_with("rebase-apply/")
        || path.starts_with("refs/bisect/")
}

fn is_current_branch_ref(repo_path: &Path, ref_path: &str) -> bool {
    let Some(branch_name) = ref_path.strip_prefix("refs/heads/") else {
        return false;
    };
    let Ok(repo) = git2::Repository::discover(repo_path) else {
        return false;
    };
    let Ok(head) = repo.head() else {
        return false;
    };
    head.shorthand().is_some_and(|head| head == branch_name)
}

pub fn watch_roots_for_snapshot(snapshot: &RepositorySnapshot) -> Vec<PathBuf> {
    let mut roots = BTreeSet::new();
    if let Some(worktree) = &snapshot.identity.worktree_root {
        roots.insert(worktree.clone());
    }
    roots.insert(snapshot.identity.git_dir.clone());
    roots.insert(snapshot.identity.common_dir.clone());
    if let Some(excludes_file) = configured_excludes_file(
        snapshot
            .identity
            .worktree_root
            .as_deref()
            .unwrap_or(&snapshot.identity.git_dir),
    ) {
        roots.insert(excludes_file);
    }
    roots.into_iter().filter(|path| path.exists()).collect()
}

fn watch_mode_for_root(_snapshot: &RepositorySnapshot, root: &Path) -> RecursiveMode {
    if root.is_file() {
        RecursiveMode::NonRecursive
    } else {
        RecursiveMode::Recursive
    }
}

fn is_configured_excludes_file(repo_path: &Path, path: &Path) -> bool {
    configured_excludes_file(repo_path).is_some_and(|excludes_file| same_path(&excludes_file, path))
}

fn configured_excludes_file(repo_path: &Path) -> Option<PathBuf> {
    let repo = git2::Repository::discover(repo_path).ok()?;
    let value = repo.config().ok()?.get_string("core.excludesfile").ok()?;
    let path = PathBuf::from(value);
    if path.is_absolute() {
        Some(path)
    } else {
        repo.workdir().map(|workdir| workdir.join(path))
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    let left = left.canonicalize().unwrap_or_else(|_| left.to_path_buf());
    let right = right.canonicalize().unwrap_or_else(|_| right.to_path_buf());
    left == right
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    use notify::event::{AccessKind, AccessMode, EventKind, Flag};
    use tempfile::TempDir;

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
    fn worktree_watch_uses_recursive_mode() {
        let repo = TestRepo::new();
        repo.write("README.md", "hello\n");
        repo.git(["add", "README.md"]);
        repo.git(["commit", "-m", "initial"]);
        let snapshot = snapshot_repository(repo.path()).unwrap();

        assert_eq!(
            watch_mode_for_root(&snapshot, repo.path()),
            RecursiveMode::Recursive
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
}
