use git2::{Repository, WorktreeLockStatus};

use super::{SnapshotError, WorktreeSummary};

pub(super) fn worktrees(repo: &Repository) -> Result<Vec<WorktreeSummary>, SnapshotError> {
    let mut worktrees = Vec::new();
    for name in repo.worktrees()?.iter().flatten() {
        let worktree = repo.find_worktree(name)?;
        let (locked, lock_reason) = match worktree.is_locked()? {
            WorktreeLockStatus::Unlocked => (false, None),
            WorktreeLockStatus::Locked(reason) => {
                (true, reason.map(|reason| reason.trim_end().to_string()))
            }
        };
        worktrees.push(WorktreeSummary {
            name: name.to_string(),
            path: worktree.path().to_path_buf(),
            locked,
            lock_reason,
        });
    }
    worktrees.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(worktrees)
}
