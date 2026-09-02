use std::path::Path;

use git2::{BranchType, ErrorCode, Repository};

use super::{CommitSummary, HeadKind, HeadState, RepositoryIdentity, SnapshotError, UpstreamState};

pub(super) fn repository_identity(repo: &Repository) -> Result<RepositoryIdentity, SnapshotError> {
    let worktree_root = repo.workdir().map(Path::to_path_buf);
    let git_dir = repo.path().to_path_buf();
    let common_dir = repo.commondir().to_path_buf();
    let is_linked_worktree = git_dir != common_dir;
    let id_path = worktree_root.as_deref().unwrap_or(repo.path());
    let id = stable_path_id(id_path);

    Ok(RepositoryIdentity {
        id,
        worktree_root,
        git_dir,
        common_dir,
        namespace: repo.namespace().map(ToString::to_string),
        is_bare: repo.is_bare(),
        is_empty: repository_is_empty(repo)?,
        is_shallow: repo.is_shallow(),
        is_linked_worktree,
    })
}

fn repository_is_empty(repo: &Repository) -> Result<bool, SnapshotError> {
    match repo.head() {
        Err(error) if error.code() == ErrorCode::UnbornBranch => {
            Ok(repo.references()?.next().transpose()?.is_none())
        }
        Ok(_) => Ok(false),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn head_state(repo: &Repository) -> Result<HeadState, SnapshotError> {
    match repo.head() {
        Ok(head) => {
            let oid = head.target().map(|oid| oid.to_string());
            if head.is_branch() {
                Ok(HeadState {
                    kind: HeadKind::Attached,
                    name: head.name().map(ToString::to_string),
                    branch: head.shorthand().map(ToString::to_string),
                    oid,
                })
            } else {
                Ok(HeadState {
                    kind: HeadKind::Detached,
                    name: head.name().map(ToString::to_string),
                    branch: None,
                    oid,
                })
            }
        }
        Err(error) if error.code() == ErrorCode::UnbornBranch => Ok(HeadState {
            kind: HeadKind::Unborn,
            name: None,
            branch: None,
            oid: None,
        }),
        Err(error) if error.code() == ErrorCode::NotFound => Ok(HeadState {
            kind: HeadKind::Missing,
            name: None,
            branch: None,
            oid: None,
        }),
        Err(error) => Err(SnapshotError::Git(error)),
    }
}

pub(super) fn head_commit(
    repo: &Repository,
    head: &HeadState,
) -> Result<Option<CommitSummary>, SnapshotError> {
    let Some(oid) = head
        .oid
        .as_ref()
        .and_then(|oid| git2::Oid::from_str(oid).ok())
    else {
        return Ok(None);
    };
    Ok(commit_summary(repo, oid))
}

pub(super) fn commit_summary(repo: &Repository, oid: git2::Oid) -> Option<CommitSummary> {
    let Ok(commit) = repo.find_commit(oid) else {
        return None;
    };
    let author = commit.author();

    Some(CommitSummary {
        oid: commit.id().to_string(),
        parent_oids: commit.parent_ids().map(|oid| oid.to_string()).collect(),
        summary: commit.summary().map(ToString::to_string),
        author_name: author.name().map(ToString::to_string),
        author_email: author.email().map(ToString::to_string),
        time_seconds: commit.time().seconds(),
    })
}

pub(super) fn upstream_state(
    repo: &Repository,
    head: &HeadState,
) -> Result<Option<UpstreamState>, SnapshotError> {
    if head.kind != HeadKind::Attached {
        return Ok(None);
    }

    let Some(branch_name) = &head.branch else {
        return Ok(None);
    };

    let branch = repo.find_branch(branch_name, BranchType::Local)?;
    let upstream = match branch.upstream() {
        Ok(upstream) => upstream,
        Err(error) if error.code() == ErrorCode::NotFound => return Ok(None),
        Err(error) => return Err(SnapshotError::Git(error)),
    };

    let name = upstream.name()?.unwrap_or("<invalid-utf8>").to_string();
    let upstream_oid = upstream.get().target();
    let head_oid = branch.get().target();
    let (ahead, behind) = match (head_oid, upstream_oid) {
        (Some(local), Some(remote)) => repo.graph_ahead_behind(local, remote)?,
        _ => (0, 0),
    };

    Ok(Some(UpstreamState {
        name,
        oid: upstream_oid.map(|oid| oid.to_string()),
        ahead,
        behind,
    }))
}

pub(super) fn stable_path_id(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}
