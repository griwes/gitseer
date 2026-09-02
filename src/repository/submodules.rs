use git2::{Repository, SubmoduleIgnore, SubmoduleStatus};

use super::{SnapshotError, SubmoduleState, SubmoduleSummary};

pub(super) fn submodules(repo: &Repository) -> Result<Vec<SubmoduleSummary>, SnapshotError> {
    if repo.is_bare() {
        return Ok(Vec::new());
    }

    let mut summaries = Vec::new();
    for submodule in repo.submodules()? {
        let name = submodule.name().unwrap_or("<invalid-utf8>").to_string();
        let status = repo.submodule_status(&name, SubmoduleIgnore::Unspecified)?;
        summaries.push(SubmoduleSummary {
            name,
            path: submodule.path().to_path_buf(),
            url: submodule.url().ok().flatten().map(ToString::to_string),
            branch: submodule.branch().ok().flatten().map(ToString::to_string),
            head_oid: submodule.head_id().map(|oid| oid.to_string()),
            index_oid: submodule.index_id().map(|oid| oid.to_string()),
            workdir_oid: submodule.workdir_id().map(|oid| oid.to_string()),
            status: submodule_state(status),
        });
    }
    summaries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(summaries)
}

pub(super) fn submodule_state(status: SubmoduleStatus) -> SubmoduleState {
    SubmoduleState {
        in_head: status.is_in_head(),
        in_index: status.is_in_index(),
        in_config: status.is_in_config(),
        in_workdir: status.is_in_wd(),
        index_added: status.is_index_added(),
        index_deleted: status.is_index_deleted(),
        index_modified: status.is_index_modified(),
        workdir_uninitialized: status.is_wd_uninitialized(),
        workdir_added: status.is_wd_added(),
        workdir_deleted: status.is_wd_deleted(),
        workdir_modified: status.is_wd_modified(),
        workdir_worktree_modified: status.is_wd_wd_modified(),
        workdir_untracked: status.is_wd_untracked(),
    }
}
