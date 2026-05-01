use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use notify::RecursiveMode;

use crate::RepositorySnapshot;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WatchTarget {
    pub path: PathBuf,
    pub mode: RecursiveMode,
}

pub fn watch_roots_for_snapshot(snapshot: &RepositorySnapshot) -> Vec<PathBuf> {
    watch_targets_for_snapshot(snapshot)
        .into_iter()
        .map(|target| target.path)
        .collect()
}

pub(super) fn watch_targets_for_snapshot(snapshot: &RepositorySnapshot) -> Vec<WatchTarget> {
    let mut targets = BTreeMap::new();
    if let Some(worktree) = &snapshot.identity.worktree_root {
        for path in worktree_watch_dirs(worktree) {
            insert_target(&mut targets, path, RecursiveMode::NonRecursive);
        }
    }
    insert_target(
        &mut targets,
        snapshot.identity.git_dir.clone(),
        RecursiveMode::Recursive,
    );
    insert_target(
        &mut targets,
        snapshot.identity.common_dir.clone(),
        RecursiveMode::Recursive,
    );
    if let Some(excludes_file) = configured_excludes_file(
        snapshot
            .identity
            .worktree_root
            .as_deref()
            .unwrap_or(&snapshot.identity.git_dir),
    ) {
        insert_target(&mut targets, excludes_file, RecursiveMode::NonRecursive);
    }
    targets
        .into_iter()
        .filter(|(path, _)| path.exists())
        .map(|(path, mode)| WatchTarget { path, mode })
        .collect()
}

#[cfg(test)]
pub(super) fn watch_mode_for_root(snapshot: &RepositorySnapshot, root: &Path) -> RecursiveMode {
    watch_targets_for_snapshot(snapshot)
        .into_iter()
        .find(|target| target.path == root)
        .map(|target| target.mode)
        .unwrap_or_else(|| {
            if root.is_file() {
                RecursiveMode::NonRecursive
            } else {
                RecursiveMode::Recursive
            }
        })
}

pub(super) fn worktree_watch_dirs(worktree: &Path) -> Vec<PathBuf> {
    worktree_watch_dirs_from(worktree, worktree)
}

pub(super) fn worktree_watch_dirs_from(worktree: &Path, start: &Path) -> Vec<PathBuf> {
    let repo = git2::Repository::discover(worktree).ok();
    if start != worktree && git_ignored(repo.as_ref(), worktree, start) {
        return Vec::new();
    }
    let mut dirs = Vec::new();
    collect_worktree_watch_dirs(repo.as_ref(), worktree, start, &mut dirs);
    dirs
}

pub(super) fn is_configured_excludes_file(repo_path: &Path, path: &Path) -> bool {
    configured_excludes_file(repo_path).is_some_and(|excludes_file| same_path(&excludes_file, path))
}

fn collect_worktree_watch_dirs(
    repo: Option<&git2::Repository>,
    worktree: &Path,
    dir: &Path,
    dirs: &mut Vec<PathBuf>,
) {
    dirs.push(dir.to_path_buf());
    if dir != worktree && dir.join(".git").exists() {
        return;
    }

    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.file_name().is_some_and(|name| name == ".git") {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() || git_ignored(repo, worktree, &path) {
            continue;
        }
        collect_worktree_watch_dirs(repo, worktree, &path, dirs);
    }
}

fn git_ignored(repo: Option<&git2::Repository>, worktree: &Path, path: &Path) -> bool {
    let Some(repo) = repo else {
        return false;
    };
    let Ok(relative) = path.strip_prefix(worktree) else {
        return false;
    };
    // Directory-only ignore rules such as `build/` may not match the directory
    // path itself through libgit2, so probe a synthetic child as well.
    let probe = relative.join("__gitseer_directory_probe__");
    repo.status_should_ignore(relative).unwrap_or(false)
        || repo.status_should_ignore(&probe).unwrap_or(false)
}

fn insert_target(
    targets: &mut BTreeMap<PathBuf, RecursiveMode>,
    path: PathBuf,
    mode: RecursiveMode,
) {
    match targets.get(&path) {
        Some(RecursiveMode::Recursive) => {}
        Some(RecursiveMode::NonRecursive) if mode == RecursiveMode::Recursive => {
            targets.insert(path, mode);
        }
        Some(RecursiveMode::NonRecursive) => {}
        None => {
            targets.insert(path, mode);
        }
    }
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
