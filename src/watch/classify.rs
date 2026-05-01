use std::path::Path;

use notify::Event;

use crate::{RefreshDomain, RefreshPlan};

use super::roots::is_configured_excludes_file;

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

pub(super) fn event_may_change_ignore_rules(
    event: &Event,
    repo_path: &Path,
    worktree_root: Option<&Path>,
    git_dir: &Path,
    common_dir: &Path,
) -> bool {
    event.paths.iter().any(|path| {
        if is_configured_excludes_file(repo_path, path) {
            return true;
        }
        if path
            .strip_prefix(git_dir)
            .or_else(|_| path.strip_prefix(common_dir))
            .is_ok_and(|relative| {
                matches!(
                    relative.to_string_lossy().as_ref(),
                    "info/exclude" | "config" | "config.worktree"
                )
            })
        {
            return true;
        }
        if let Some(worktree_root) = worktree_root
            && let Ok(relative) = path.strip_prefix(worktree_root)
        {
            return is_ignore_rule_path(relative);
        }
        false
    })
}

fn is_mutating_event(event: &Event) -> bool {
    !matches!(
        event.kind,
        notify::event::EventKind::Access(
            notify::event::AccessKind::Read
                | notify::event::AccessKind::Open(_)
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

    match git2::Repository::discover(repo_path) {
        Ok(repo) if path_should_be_ignored(&repo, relative) => RefreshPlan::None,
        Ok(_) => RefreshPlan::domains([RefreshDomain::Paths]),
        Err(_) => RefreshPlan::domains([RefreshDomain::Paths]),
    }
}

fn path_should_be_ignored(repo: &git2::Repository, relative: &Path) -> bool {
    // Directory-only ignore rules such as `build/` may not match the directory
    // path itself through libgit2, so probe a synthetic child as well.
    let probe = relative.join("__gitseer_directory_probe__");
    repo.status_should_ignore(relative).unwrap_or(false)
        || repo.status_should_ignore(&probe).unwrap_or(false)
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
    let text = match lock_target_for_git_path(&text) {
        LockTarget::Known(target) => target,
        LockTarget::Unmodeled => return RefreshPlan::None,
        LockTarget::NotLock => &text,
    };

    if text == "index"
        || text.starts_with("index.")
        || text == "info/exclude"
        || text == "info/sparse-checkout"
        || text == "config.worktree"
    {
        return RefreshPlan::domains([RefreshDomain::Paths]);
    }
    if text.starts_with("objects/") {
        return RefreshPlan::None;
    }
    if text == "modules" || text.starts_with("modules/") {
        return RefreshPlan::domains([RefreshDomain::Paths, RefreshDomain::Submodules]);
    }
    if text.starts_with("rr-cache/") {
        return RefreshPlan::None;
    }
    if matches!(text, "COMMIT_EDITMSG" | "AUTO_MERGE" | "MERGE_RR") {
        return RefreshPlan::None;
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
    if text == "refs/heads" {
        return RefreshPlan::domains([
            RefreshDomain::Head,
            RefreshDomain::Upstream,
            RefreshDomain::Branches,
        ]);
    }
    if text == "refs/bisect" {
        return RefreshPlan::domains([
            RefreshDomain::Operation,
            RefreshDomain::Head,
            RefreshDomain::Upstream,
            RefreshDomain::Branches,
            RefreshDomain::Paths,
        ]);
    }
    if text.starts_with("refs/heads/") {
        let mut domains = vec![
            RefreshDomain::Head,
            RefreshDomain::Upstream,
            RefreshDomain::Branches,
        ];
        if is_current_branch_ref(repo_path, text) {
            domains.push(RefreshDomain::Paths);
        }
        return RefreshPlan::domains(domains);
    }
    if text == "refs/remotes" || text.starts_with("refs/remotes/") || text == "FETCH_HEAD" {
        return RefreshPlan::domains([
            RefreshDomain::Upstream,
            RefreshDomain::Branches,
            RefreshDomain::Remotes,
        ]);
    }
    if text == "refs/tags" || text.starts_with("refs/tags/") {
        return RefreshPlan::domains([RefreshDomain::Tags]);
    }
    if text == "packed-refs" || text == "packed-refs.new" {
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
    if text.starts_with("logs/") {
        return RefreshPlan::None;
    }
    if text == "worktrees"
        || text.starts_with("worktrees/")
        || text == "commondir"
        || text == "gitdir"
    {
        return RefreshPlan::domains([
            RefreshDomain::Identity,
            RefreshDomain::Upstream,
            RefreshDomain::Branches,
            RefreshDomain::Worktrees,
        ]);
    }
    if is_operation_path(text) {
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
            | "BISECT_ANCESTORS_OK"
            | "BISECT_EXPECTED_REV"
            | "BISECT_HEAD"
            | "BISECT_NAMES"
            | "BISECT_START"
            | "BISECT_TERMS"
            | "MERGE_MODE"
            | "MERGE_MSG"
            | "rebase-merge"
            | "rebase-apply"
            | "sequencer"
    ) || path.starts_with("sequencer/")
        || path.starts_with("rebase-merge/")
        || path.starts_with("rebase-apply/")
        || path.starts_with("refs/bisect/")
}

enum LockTarget<'a> {
    Known(&'a str),
    Unmodeled,
    NotLock,
}

fn lock_target_for_git_path(path: &str) -> LockTarget<'_> {
    let Some(target) = path.strip_suffix(".lock") else {
        return LockTarget::NotLock;
    };

    if target == "HEAD"
        || target == "ORIG_HEAD"
        || target == "index"
        || target == "config"
        || target == "config.worktree"
        || target == "info/sparse-checkout"
        || target.starts_with("index.")
        || target.starts_with("refs/")
        || target.starts_with("worktrees/")
        || is_operation_path(target)
    {
        return LockTarget::Known(target);
    }

    LockTarget::Unmodeled
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
