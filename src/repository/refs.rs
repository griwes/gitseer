use git2::{BranchType, ErrorCode, ObjectType, Repository};

use super::identity::commit_summary;
use super::{
    BranchKind, BranchSummary, GitObjectKind, RemoteSummary, SnapshotError, StashSummary, TagKind,
    TagSummary,
};

pub(super) fn remotes(repo: &Repository) -> Result<Vec<RemoteSummary>, SnapshotError> {
    let mut remotes = Vec::new();
    for name in repo.remotes()?.iter().flatten() {
        let remote = repo.find_remote(name)?;
        remotes.push(RemoteSummary {
            name: name.to_string(),
            url: remote.url().map(ToString::to_string),
            push_url: remote.pushurl().map(ToString::to_string),
            default_branch: remote_default_branch(repo, name),
            fetch_refspecs: remote
                .fetch_refspecs()?
                .iter()
                .flatten()
                .map(ToString::to_string)
                .collect(),
            push_refspecs: remote
                .push_refspecs()?
                .iter()
                .flatten()
                .map(ToString::to_string)
                .collect(),
        });
    }
    remotes.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(remotes)
}

pub(super) fn remote_default_branch(repo: &Repository, remote_name: &str) -> Option<String> {
    let reference = repo
        .find_reference(&format!("refs/remotes/{remote_name}/HEAD"))
        .ok()?;
    let target = reference.symbolic_target()?;
    target
        .strip_prefix(&format!("refs/remotes/{remote_name}/"))
        .map(ToString::to_string)
}

pub(super) fn branches(repo: &Repository) -> Result<Vec<BranchSummary>, SnapshotError> {
    let mut branches = Vec::new();
    for branch in repo.branches(None)? {
        let (branch, kind) = branch?;
        let name = branch.name()?.unwrap_or("<invalid-utf8>").to_string();
        let upstream = if kind == BranchType::Local {
            match branch.upstream() {
                Ok(upstream) => upstream.name()?.map(ToString::to_string),
                Err(error) if error.code() == ErrorCode::NotFound => None,
                Err(error) => return Err(SnapshotError::Git(error)),
            }
        } else {
            None
        };
        let (upstream_ahead, upstream_behind) = branch_upstream_counts(repo, &branch)?;

        branches.push(BranchSummary {
            name,
            kind: match kind {
                BranchType::Local => BranchKind::Local,
                BranchType::Remote => BranchKind::Remote,
            },
            is_head: branch.is_head(),
            oid: branch.get().target().map(|oid| oid.to_string()),
            tip_commit: branch
                .get()
                .target()
                .and_then(|oid| commit_summary(repo, oid)),
            upstream,
            upstream_ahead,
            upstream_behind,
        });
    }
    branches.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.kind.cmp(&b.kind)));
    Ok(branches)
}

pub(super) fn branch_upstream_counts(
    repo: &Repository,
    branch: &git2::Branch<'_>,
) -> Result<(Option<usize>, Option<usize>), SnapshotError> {
    let Ok(upstream) = branch.upstream() else {
        return Ok((None, None));
    };
    let Some(local_oid) = branch.get().target() else {
        return Ok((None, None));
    };
    let Some(upstream_oid) = upstream.get().target() else {
        return Ok((None, None));
    };
    let (ahead, behind) = repo.graph_ahead_behind(local_oid, upstream_oid)?;
    Ok((Some(ahead), Some(behind)))
}

pub(super) fn tags(repo: &Repository) -> Result<Vec<TagSummary>, SnapshotError> {
    let mut tags = Vec::new();
    for name in repo.tag_names(None)?.iter().flatten() {
        let object = repo.revparse_single(&format!("refs/tags/{name}"))?;
        let summary = if let Some(tag) = object.as_tag() {
            let tagger = tag.tagger();
            TagSummary {
                name: name.to_string(),
                oid: tag.id().to_string(),
                kind: TagKind::Annotated,
                target_oid: tag.target_id().to_string(),
                target_kind: tag.target_type().and_then(git_object_kind),
                tagger_name: tagger
                    .as_ref()
                    .and_then(|tagger| tagger.name().map(ToString::to_string)),
                tagger_email: tagger
                    .as_ref()
                    .and_then(|tagger| tagger.email().map(ToString::to_string)),
                tagger_time_seconds: tagger.as_ref().map(|tagger| tagger.when().seconds()),
                message: tag.message().map(ToString::to_string),
            }
        } else {
            TagSummary {
                name: name.to_string(),
                oid: object.id().to_string(),
                kind: TagKind::Lightweight,
                target_oid: object.id().to_string(),
                target_kind: object.kind().and_then(git_object_kind),
                tagger_name: None,
                tagger_email: None,
                tagger_time_seconds: None,
                message: None,
            }
        };
        tags.push(summary);
    }
    tags.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(tags)
}

pub(super) fn git_object_kind(kind: ObjectType) -> Option<GitObjectKind> {
    match kind {
        ObjectType::Commit => Some(GitObjectKind::Commit),
        ObjectType::Tree => Some(GitObjectKind::Tree),
        ObjectType::Blob => Some(GitObjectKind::Blob),
        ObjectType::Tag => Some(GitObjectKind::Tag),
        ObjectType::Any => None,
    }
}

pub(super) fn stashes(repo: &mut Repository) -> Result<Vec<StashSummary>, SnapshotError> {
    let mut stashes = Vec::new();
    repo.stash_foreach(|index, message, oid| {
        stashes.push(StashSummary {
            index,
            message: message.to_string(),
            oid: oid.to_string(),
        });
        true
    })?;
    Ok(stashes)
}
