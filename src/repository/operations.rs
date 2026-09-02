use std::fs;

use git2::{Repository, RepositoryState};

use super::{BisectState, OperationHead, OperationHeadRole, OperationKind, OperationState};

pub(super) fn operation_state(repo: &Repository) -> OperationState {
    let kind = match repo.state() {
        RepositoryState::Clean => OperationKind::Clean,
        RepositoryState::Merge => OperationKind::Merge,
        RepositoryState::Revert => OperationKind::Revert,
        RepositoryState::RevertSequence => OperationKind::RevertSequence,
        RepositoryState::CherryPick => OperationKind::CherryPick,
        RepositoryState::CherryPickSequence => OperationKind::CherryPickSequence,
        RepositoryState::Bisect => OperationKind::Bisect,
        RepositoryState::Rebase => OperationKind::Rebase,
        RepositoryState::RebaseInteractive => OperationKind::RebaseInteractive,
        RepositoryState::RebaseMerge => OperationKind::RebaseMerge,
        RepositoryState::ApplyMailbox => OperationKind::ApplyMailbox,
        RepositoryState::ApplyMailboxOrRebase => OperationKind::ApplyMailboxOrRebase,
    };

    OperationState {
        bisect: (kind == OperationKind::Bisect).then(|| bisect_state(repo)),
        kind,
        message: repo.message().ok(),
        heads: operation_heads(repo),
    }
}

pub(super) fn operation_heads(repo: &Repository) -> Vec<OperationHead> {
    let mut heads = Vec::new();
    for (relative_path, role) in [
        ("MERGE_HEAD", OperationHeadRole::Merge),
        ("REBASE_HEAD", OperationHeadRole::Rebase),
        ("CHERRY_PICK_HEAD", OperationHeadRole::CherryPick),
        ("REVERT_HEAD", OperationHeadRole::Revert),
    ] {
        read_operation_head_file(repo, relative_path, role, &mut heads);
    }
    heads.sort_by(|a, b| a.role.cmp(&b.role).then_with(|| a.oid.cmp(&b.oid)));
    heads
}

pub(super) fn read_operation_head_file(
    repo: &Repository,
    relative_path: &str,
    role: OperationHeadRole,
    heads: &mut Vec<OperationHead>,
) {
    let Ok(contents) = fs::read_to_string(repo.path().join(relative_path)) else {
        return;
    };

    for oid in contents
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|oid| git2::Oid::from_str(oid).is_ok())
    {
        heads.push(OperationHead {
            role,
            oid: oid.to_string(),
        });
    }
}

pub(super) fn bisect_state(repo: &Repository) -> BisectState {
    let mut state = BisectState {
        start_oid: read_operation_oid_file(repo, "BISECT_START"),
        good_oids: Vec::new(),
        bad_oids: Vec::new(),
        skipped_oids: Vec::new(),
    };

    let Ok(references) = repo.references() else {
        return state;
    };

    for reference in references.flatten() {
        let Ok(name) = reference.name() else {
            continue;
        };
        let Some(oid) = reference.target().map(|oid| oid.to_string()) else {
            continue;
        };

        if name == "refs/bisect/bad" {
            state.bad_oids.push(oid);
        } else if name.starts_with("refs/bisect/good-") {
            state.good_oids.push(oid);
        } else if name.starts_with("refs/bisect/skip-") {
            state.skipped_oids.push(oid);
        }
    }

    state.good_oids.sort();
    state.bad_oids.sort();
    state.skipped_oids.sort();
    state
}

pub(super) fn read_operation_oid_file(repo: &Repository, relative_path: &str) -> Option<String> {
    let contents = fs::read_to_string(repo.path().join(relative_path)).ok()?;
    let token = contents.split_whitespace().next()?;
    if git2::Oid::from_str(token).is_ok() {
        Some(token.to_string())
    } else {
        repo.find_reference(token)
            .ok()
            .and_then(|reference| reference.target())
            .map(|oid| oid.to_string())
    }
}
