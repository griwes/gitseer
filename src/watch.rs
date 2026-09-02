use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use thiserror::Error;
use tokio::sync::mpsc;

use crate::{RefreshPlan, SnapshotError, snapshot_repository};

mod classify;
mod roots;

pub const MAX_DEBOUNCE_DRAIN_EVENTS: usize = 256;
const MAX_PENDING_WATCH_EVENTS: usize = 4096;

pub use classify::{refresh_plan_for_event, should_refresh_for_event};
pub use roots::watch_roots_for_snapshot;

use roots::{WatchTarget, watch_targets_for_snapshot, worktree_watch_dirs_from};
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
    queue_overflowed: Arc<AtomicBool>,
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
        let watch_targets = watch_targets_for_snapshot(&snapshot);
        // Git operations can produce several hundred useful events before the
        // async consumer gets a turn. Keep that burst bounded independently of
        // the smaller per-turn debounce budget.
        let (tx, rx) = mpsc::channel(MAX_PENDING_WATCH_EVENTS);
        let queue_overflowed = Arc::new(AtomicBool::new(false));
        let callback_overflowed = Arc::clone(&queue_overflowed);
        let mut watcher = notify::recommended_watcher(move |event| {
            if !classify::should_refresh_for_event(&event) {
                return;
            }
            enqueue_event(&tx, &callback_overflowed, event);
        })?;

        let mut watched_roots = Vec::with_capacity(watch_targets.len());
        for target in watch_targets {
            watcher.watch(&target.path, target.mode)?;
            watched_roots.push(target.path);
        }

        Ok(Self {
            _watcher: watcher,
            rx,
            queue_overflowed,
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
        if self.take_queue_overflow() {
            return Some(Err(notify::Error::generic(
                "repository watcher event queue overflowed",
            )));
        }
        let event = self.rx.recv().await;
        if let Some(event) = &event {
            self.update_watches_for_event(event).await;
        }
        event
    }

    fn take_queue_overflow(&self) -> bool {
        self.queue_overflowed.swap(false, Ordering::AcqRel)
    }

    pub async fn debounce_plan(&mut self, initial: RefreshPlan, duration: Duration) -> RefreshPlan {
        tokio::time::sleep(duration).await;
        drain_pending_plan(self, initial).await
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

fn enqueue_event(
    tx: &mpsc::Sender<notify::Result<Event>>,
    queue_overflowed: &AtomicBool,
    event: notify::Result<Event>,
) {
    if matches!(tx.try_send(event), Err(mpsc::error::TrySendError::Full(_))) {
        queue_overflowed.store(true, Ordering::Release);
    }
}

impl RepositoryWatcher {
    async fn update_watches_for_event(&mut self, event: &notify::Result<Event>) {
        let Ok(event) = event else {
            return;
        };
        if event.need_rescan() {
            self.reconcile_watch_targets().await;
            return;
        }
        if classify::event_may_change_ignore_rules(
            event,
            &self.repo_path,
            self.worktree_root.as_deref(),
            &self.git_dir,
            &self.common_dir,
        ) {
            self.reconcile_watch_targets().await;
            return;
        }
        self.add_worktree_watches_for_event(event).await;
    }

    async fn reconcile_watch_targets(&mut self) {
        let repo_path = self.repo_path.clone();
        let Ok(Ok((targets, worktree_root, git_dir, common_dir))) =
            tokio::task::spawn_blocking(move || {
                let snapshot = snapshot_repository(&repo_path)?;
                Ok::<_, SnapshotError>((
                    watch_targets_for_snapshot(&snapshot),
                    snapshot.identity.worktree_root,
                    snapshot.identity.git_dir,
                    snapshot.identity.common_dir,
                ))
            })
            .await
        else {
            return;
        };

        self.worktree_root = worktree_root;
        self.git_dir = git_dir;
        self.common_dir = common_dir;
        self.replace_watch_targets(targets);
    }

    async fn add_worktree_watches_for_event(&mut self, event: &Event) {
        let Some(worktree_root) = self.worktree_root.clone() else {
            return;
        };
        if !event_can_create_directory_watch(event) {
            return;
        }
        for path in &event.paths {
            self.add_worktree_watch_if_needed(&worktree_root, path)
                .await;
        }
    }

    async fn add_worktree_watch_if_needed(&mut self, worktree_root: &Path, path: &Path) {
        if path.starts_with(&self.git_dir) || path.starts_with(&self.common_dir) {
            return;
        }
        if !path.is_dir() || !path.starts_with(worktree_root) {
            return;
        }
        self.prune_stale_worktree_watch_roots(worktree_root);
        if self.watched_roots.iter().any(|watched| watched == path) {
            return;
        }
        let worktree_root = worktree_root.to_path_buf();
        let path = path.to_path_buf();
        let Ok(dirs) =
            tokio::task::spawn_blocking(move || worktree_watch_dirs_from(&worktree_root, &path))
                .await
        else {
            return;
        };

        for dir in dirs {
            self.watch_root(dir, RecursiveMode::NonRecursive);
        }
    }

    fn replace_watch_targets(&mut self, targets: Vec<WatchTarget>) {
        let desired = targets
            .iter()
            .map(|target| target.path.clone())
            .collect::<BTreeSet<_>>();
        let current = self.watched_roots.clone();
        for path in current {
            if !desired.contains(&path) {
                let _ = self._watcher.unwatch(&path);
            }
        }

        self.watched_roots.retain(|path| desired.contains(path));
        for target in targets {
            self.watch_root(target.path, target.mode);
        }
    }

    fn watch_root(&mut self, path: PathBuf, mode: RecursiveMode) {
        if self.watched_roots.iter().any(|watched| watched == &path) {
            return;
        }
        if self._watcher.watch(&path, mode).is_ok() {
            self.watched_roots.push(path);
        }
    }

    fn prune_stale_worktree_watch_roots(&mut self, worktree_root: &Path) {
        self.watched_roots.retain(|path| {
            if path.starts_with(worktree_root) && !path.exists() {
                let _ = self._watcher.unwatch(path);
                false
            } else {
                true
            }
        });
    }
}

fn event_can_create_directory_watch(event: &Event) -> bool {
    use notify::event::{CreateKind, EventKind, ModifyKind};

    matches!(
        event.kind,
        EventKind::Any
            | EventKind::Create(CreateKind::Any | CreateKind::Folder)
            | EventKind::Modify(ModifyKind::Any | ModifyKind::Name(_))
    )
}

async fn drain_pending_plan(watcher: &mut RepositoryWatcher, initial: RefreshPlan) -> RefreshPlan {
    if watcher.take_queue_overflow() {
        return RefreshPlan::Full;
    }
    let mut plan = initial;
    let mut drained = 0;
    while let Ok(event) = watcher.rx.try_recv() {
        watcher.update_watches_for_event(&event).await;
        plan = plan.combine(watcher.refresh_plan_for_event(&event));
        drained += 1;
        // Avoid self-sustaining metadata event storms. Any remaining events stay
        // queued for the next watcher turn.
        if drained >= MAX_DEBOUNCE_DRAIN_EVENTS {
            break;
        }
    }
    if watcher.take_queue_overflow() {
        RefreshPlan::Full
    } else {
        plan
    }
}

#[cfg(test)]
mod tests;
