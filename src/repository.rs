use std::collections::BTreeSet;
use std::path::Path;

use git2::{ErrorCode, Repository};

mod identity;
mod model;
mod operations;
mod paths;
mod refs;
mod submodules;
mod worktrees;

pub use model::*;

use identity::{head_commit, head_state, repository_identity, upstream_state};
use operations::operation_state;
use paths::{path_delta, path_state};
use refs::{branches, remotes, stashes, tags};
use submodules::submodules;
use worktrees::worktrees;
pub fn snapshot_repository(path: impl AsRef<Path>) -> Result<RepositorySnapshot, SnapshotError> {
    snapshot_repository_with_options(path, SnapshotOptions::default())
}

pub fn snapshot_repository_with_options(
    path: impl AsRef<Path>,
    options: SnapshotOptions,
) -> Result<RepositorySnapshot, SnapshotError> {
    let path = path.as_ref();
    let mut repo = Repository::discover(path).map_err(|error| {
        if error.code() == ErrorCode::NotFound {
            SnapshotError::NotRepository {
                path: path.to_path_buf(),
            }
        } else {
            SnapshotError::Git(error)
        }
    })?;

    let identity = repository_identity(&repo)?;
    let head = head_state(&repo)?;
    let head_commit = head_commit(&repo, &head)?;
    let upstream = upstream_state(&repo, &head)?;
    let paths = path_state(&repo, options)?;
    let operation = operation_state(&repo);
    let remotes = remotes(&repo)?;
    let branches = branches(&repo)?;
    let tags = tags(&repo)?;
    let stashes = stashes(&mut repo)?;
    let worktrees = worktrees(&repo)?;
    let submodules = submodules(&repo)?;

    Ok(RepositorySnapshot {
        identity,
        head,
        head_commit,
        upstream,
        paths,
        operation,
        remotes,
        branches,
        tags,
        stashes,
        worktrees,
        submodules,
    })
}

pub fn refresh_repository_with_plan(
    path: impl AsRef<Path>,
    previous: Option<&RepositorySnapshot>,
    plan: &RefreshPlan,
    options: SnapshotOptions,
) -> Result<SnapshotRefresh, SnapshotError> {
    let Some(previous) = previous else {
        return Ok(SnapshotRefresh {
            snapshot: snapshot_repository_with_options(path, options)?,
            plan: RefreshPlan::Full,
        });
    };

    match plan {
        RefreshPlan::None => Ok(SnapshotRefresh {
            snapshot: previous.clone(),
            plan: RefreshPlan::None,
        }),
        RefreshPlan::Full => Ok(SnapshotRefresh {
            snapshot: snapshot_repository_with_options(path, options)?,
            plan: RefreshPlan::Full,
        }),
        RefreshPlan::Domains(domains) => {
            let path = path.as_ref();
            let mut repo = Repository::discover(path).map_err(|error| {
                if error.code() == ErrorCode::NotFound {
                    SnapshotError::NotRepository {
                        path: path.to_path_buf(),
                    }
                } else {
                    SnapshotError::Git(error)
                }
            })?;
            let mut snapshot = previous.clone();
            refresh_domains(&mut repo, &mut snapshot, domains, options)?;
            Ok(SnapshotRefresh {
                snapshot,
                plan: plan.clone(),
            })
        }
    }
}

pub fn snapshot_delta(
    previous: &RepositorySnapshot,
    current: &RepositorySnapshot,
) -> SnapshotDelta {
    SnapshotDelta {
        repository_id: current.identity.id.clone(),
        identity_changed: previous.identity != current.identity,
        head_changed: previous.head != current.head || previous.head_commit != current.head_commit,
        upstream_changed: previous.upstream != current.upstream,
        operation_changed: previous.operation != current.operation,
        paths: path_delta(&previous.paths, &current.paths),
        remotes_changed: previous.remotes != current.remotes,
        branches_changed: previous.branches != current.branches,
        tags_changed: previous.tags != current.tags,
        stashes_changed: previous.stashes != current.stashes,
        worktrees_changed: previous.worktrees != current.worktrees,
        submodules_changed: previous.submodules != current.submodules,
    }
}

fn refresh_domains(
    repo: &mut Repository,
    snapshot: &mut RepositorySnapshot,
    domains: &BTreeSet<RefreshDomain>,
    options: SnapshotOptions,
) -> Result<(), SnapshotError> {
    if domains.contains(&RefreshDomain::Identity) {
        snapshot.identity = repository_identity(repo)?;
    }
    if domains.contains(&RefreshDomain::Head) {
        snapshot.head = head_state(repo)?;
        snapshot.head_commit = head_commit(repo, &snapshot.head)?;
    }
    if domains.contains(&RefreshDomain::Upstream) {
        snapshot.upstream = upstream_state(repo, &snapshot.head)?;
    }
    if domains.contains(&RefreshDomain::Paths) {
        snapshot.paths = path_state(repo, options)?;
    }
    if domains.contains(&RefreshDomain::Operation) {
        snapshot.operation = operation_state(repo);
    }
    if domains.contains(&RefreshDomain::Remotes) {
        snapshot.remotes = remotes(repo)?;
    }
    if domains.contains(&RefreshDomain::Branches) {
        snapshot.branches = branches(repo)?;
    }
    if domains.contains(&RefreshDomain::Tags) {
        snapshot.tags = tags(repo)?;
    }
    if domains.contains(&RefreshDomain::Stashes) {
        snapshot.stashes = stashes(repo)?;
    }
    if domains.contains(&RefreshDomain::Worktrees) {
        snapshot.worktrees = worktrees(repo)?;
    }
    if domains.contains(&RefreshDomain::Submodules) {
        snapshot.submodules = submodules(repo)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
