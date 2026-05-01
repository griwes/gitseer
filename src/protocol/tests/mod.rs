use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use notify::Event;
use notify::event::EventKind;
use serde_json::json;
use tempfile::TempDir;

use crate::{
    HeadKind, RefreshPlan, RepositoryWatcher, refresh_repository_with_plan,
    watch::MAX_DEBOUNCE_DRAIN_EVENTS,
};

use super::*;

mod command_ops_bisect;
mod command_ops_cherry_pick;
mod command_ops_merge;
mod command_ops_rebase;
mod command_ops_revert;
mod command_paths_ignores;
mod command_paths_worktree;
mod command_refs_branches;
mod command_refs_remote_config;
mod command_refs_stashes;
mod command_refs_tags;
mod command_refs_upstreams;
mod command_topology_repository_modes;
mod command_topology_shallow_sparse;
mod command_topology_submodules;
mod command_topology_worktrees;
mod core;
mod wire;

thread_local! {
    static LIVE_WATCHERS: RefCell<HashMap<usize, RepositoryWatcher>> = RefCell::new(HashMap::new());
}

fn subscribe_for_deltas(state: &mut ProcessState) -> SnapshotNotificationParams {
    let messages = handle_request(
        state,
        r#"{"jsonrpc":"2.0","id":"subscribe","method":"gitseer/subscribe"}"#,
    );
    assert_eq!(messages.len(), 2);
    assert!(matches!(messages[0], ServerMessage::Response(_)));
    let params = match &messages[1] {
        ServerMessage::Notification(notification) => {
            assert_eq!(notification.method, "gitseer/snapshot");
            serde_json::from_value(notification.params.clone().unwrap()).unwrap()
        }
        ServerMessage::Response(_) => panic!("expected snapshot notification"),
    };
    start_live_watcher(state);
    params
}

fn update_from_watch_event(
    state: &mut ProcessState,
    event: Event,
) -> (RefreshPlan, DeltaNotificationParams) {
    let plan = next_live_refresh_plan(state, &event);
    let previous = state.latest_snapshot().unwrap().clone();
    let refresh = refresh_repository_with_plan(
        state.repo(),
        Some(&previous),
        &plan,
        SnapshotOptions::default(),
    )
    .unwrap();
    assert_eq!(refresh.plan, plan);
    let messages = snapshot_update_messages(state, refresh.snapshot);
    (plan, only_delta_notification(messages))
}

fn update_from_watch_event_with_no_delta(state: &mut ProcessState, event: Event) -> RefreshPlan {
    let plan = next_live_plan_allowing_no_refresh(state, &event);
    let previous = state.latest_snapshot().unwrap().clone();
    let refresh = refresh_repository_with_plan(
        state.repo(),
        Some(&previous),
        &plan,
        SnapshotOptions::default(),
    )
    .unwrap();
    assert_eq!(refresh.plan, plan);
    let messages = snapshot_update_messages(state, refresh.snapshot);
    assert!(messages.is_empty());
    plan
}

fn start_live_watcher(state: &ProcessState) {
    let key = state as *const ProcessState as usize;
    let watcher = RepositoryWatcher::new(state.repo()).unwrap();
    LIVE_WATCHERS.with(|watchers| {
        watchers.borrow_mut().insert(key, watcher);
    });
    std::thread::sleep(Duration::from_millis(25));
}

fn next_live_refresh_plan(state: &ProcessState, diagnostic_event: &Event) -> RefreshPlan {
    let plan = next_live_plan(state, diagnostic_event, true);
    assert!(
        plan.should_refresh(),
        "live watcher saw events for {:?}, but none required a refresh",
        diagnostic_event.paths
    );
    plan
}

fn assert_incremental_refresh_plan(plan: &RefreshPlan) {
    assert!(
        plan.should_refresh(),
        "real watcher notification did not request a refresh"
    );
    assert_ne!(
        plan,
        &RefreshPlan::Full,
        "real watcher notification fell back to a full rescan"
    );
}

fn next_live_plan_allowing_no_refresh(
    state: &ProcessState,
    diagnostic_event: &Event,
) -> RefreshPlan {
    next_live_plan(state, diagnostic_event, false)
}

fn next_live_plan(
    state: &ProcessState,
    diagnostic_event: &Event,
    require_refresh: bool,
) -> RefreshPlan {
    let key = state as *const ProcessState as usize;
    LIVE_WATCHERS.with(|watchers| {
        let mut watchers = watchers.borrow_mut();
        let watcher = watchers
            .get_mut(&key)
            .expect("subscribe_for_deltas must be called before waiting for live events");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        runtime.block_on(async {
            let wait = if require_refresh {
                Duration::from_secs(5)
            } else {
                Duration::from_millis(500)
            };
            let deadline = tokio::time::Instant::now() + wait;
            let mut plan = RefreshPlan::None;
            let mut saw_event = false;
            while !saw_event {
                let now = tokio::time::Instant::now();
                if now >= deadline {
                    if require_refresh {
                        panic!(
                            "timed out waiting for live watcher event; expected activity near {:?}",
                            diagnostic_event.paths
                        );
                    }
                    return plan;
                }
                let remaining = deadline - now;
                let event = match tokio::time::timeout(remaining, watcher.next_event()).await {
                    Ok(Some(event)) => event,
                    Ok(None) => panic!("watcher stopped"),
                    Err(_) if !require_refresh => return plan,
                    Err(_) => panic!(
                        "timed out waiting for live watcher event; expected activity near {:?}",
                        diagnostic_event.paths
                    ),
                };
                saw_event = true;
                plan = plan.combine(watcher.refresh_plan_for_event(&event));
            }

            let mut drained = 0;
            loop {
                match tokio::time::timeout(Duration::from_millis(50), watcher.next_event()).await {
                    Ok(Some(event)) => {
                        plan = plan.combine(watcher.refresh_plan_for_event(&event));
                        drained += 1;
                        if drained >= MAX_DEBOUNCE_DRAIN_EVENTS {
                            break;
                        }
                    }
                    Ok(None) => panic!("watcher stopped"),
                    Err(_) => break,
                }
            }

            if require_refresh {
                assert!(
                    plan.should_refresh(),
                    "live watcher saw events for {:?}, but produced no refresh plan",
                    diagnostic_event.paths
                );
            }
            plan
        })
    })
}

fn only_delta_notification(messages: Vec<ServerMessage>) -> DeltaNotificationParams {
    assert_eq!(messages.len(), 1);
    match messages.into_iter().next().unwrap() {
        ServerMessage::Notification(notification) => {
            assert_eq!(notification.method, "gitseer/delta");
            serde_json::from_value(notification.params.unwrap()).unwrap()
        }
        ServerMessage::Response(_) => panic!("expected delta notification"),
    }
}

fn apply_patch_to_snapshot(
    mut snapshot: RepositorySnapshot,
    patch: SnapshotPatch,
) -> RepositorySnapshot {
    if let Some(identity) = patch.identity {
        snapshot.identity = identity;
    }
    if let Some(head) = patch.head {
        snapshot.head = head;
    }
    if let Some(head_commit) = patch.head_commit {
        snapshot.head_commit = head_commit;
    }
    if let Some(upstream) = patch.upstream {
        snapshot.upstream = upstream;
    }
    if let Some(paths) = patch.paths {
        snapshot.paths = paths;
    }
    if let Some(operation) = patch.operation {
        snapshot.operation = operation;
    }
    if let Some(remotes) = patch.remotes {
        snapshot.remotes = remotes;
    }
    if let Some(branches) = patch.branches {
        snapshot.branches = branches;
    }
    if let Some(tags) = patch.tags {
        snapshot.tags = tags;
    }
    if let Some(stashes) = patch.stashes {
        snapshot.stashes = stashes;
    }
    if let Some(worktrees) = patch.worktrees {
        snapshot.worktrees = worktrees;
    }
    if let Some(submodules) = patch.submodules {
        snapshot.submodules = submodules;
    }
    snapshot
}

fn event_for(path: impl Into<PathBuf>) -> Event {
    let mut event = Event::new(EventKind::Any);
    event.paths.push(path.into());
    event
}

fn init_bare_repo(path: &Path) {
    let output = Command::new("git")
        .args(["init", "--bare", path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git command failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_in<const N: usize>(path: &Path, args: [&str; N]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git command failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_allow_file_protocol_in<const N: usize>(path: &Path, args: [&str; N]) {
    let output = Command::new("git")
        .arg("-c")
        .arg("protocol.file.allow=always")
        .args(args)
        .current_dir(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git command failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_stdout_in<const N: usize>(path: &Path, args: [&str; N]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git command failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn only_response(messages: Vec<ServerMessage>) -> JsonRpcResponse {
    assert_eq!(messages.len(), 1);
    match messages.into_iter().next().unwrap() {
        ServerMessage::Response(response) => response,
        ServerMessage::Notification(_) => panic!("expected response"),
    }
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
        repo.git(["config", "tag.gpgsign", "false"]);
        repo.git(["config", "core.editor", "true"]);
        repo
    }

    fn clone_from(remote: &Path) -> Self {
        let temp = TempDir::new().unwrap();
        let output = Command::new("git")
            .arg("clone")
            .arg(remote)
            .arg(temp.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git clone failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let repo = Self { temp };
        repo.git(["config", "user.email", "tester@example.com"]);
        repo.git(["config", "user.name", "Tester"]);
        repo.git(["config", "commit.gpgsign", "false"]);
        repo.git(["config", "tag.gpgsign", "false"]);
        repo.git(["config", "core.editor", "true"]);
        repo
    }

    fn shallow_clone_from(remote: &Path) -> Self {
        let temp = TempDir::new().unwrap();
        let remote_url = format!("file://{}", remote.to_string_lossy());
        let output = Command::new("git")
            .arg("-c")
            .arg("protocol.file.allow=always")
            .args(["clone", "--depth", "1", &remote_url])
            .arg(temp.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git clone failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let repo = Self { temp };
        repo.git(["config", "user.email", "tester@example.com"]);
        repo.git(["config", "user.name", "Tester"]);
        repo.git(["config", "commit.gpgsign", "false"]);
        repo.git(["config", "tag.gpgsign", "false"]);
        repo.git(["config", "core.editor", "true"]);
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

    fn git_allow_file_protocol<const N: usize>(&self, args: [&str; N]) {
        let output = Command::new("git")
            .arg("-c")
            .arg("protocol.file.allow=always")
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

    fn git_expect_failure<const N: usize>(&self, args: [&str; N]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(self.path())
            .output()
            .unwrap();
        assert!(
            !output.status.success(),
            "git command unexpectedly succeeded\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_stdout<const N: usize>(&self, args: [&str; N]) -> String {
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
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }
}
