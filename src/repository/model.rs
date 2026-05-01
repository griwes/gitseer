use std::collections::BTreeSet;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SnapshotError {
    #[error("path is not inside a Git repository: {path}")]
    NotRepository { path: PathBuf },
    #[error("Git error while reading repository state: {0}")]
    Git(#[from] git2::Error),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositorySnapshot {
    pub identity: RepositoryIdentity,
    pub head: HeadState,
    pub head_commit: Option<CommitSummary>,
    pub upstream: Option<UpstreamState>,
    pub paths: PathState,
    pub operation: OperationState,
    pub remotes: Vec<RemoteSummary>,
    pub branches: Vec<BranchSummary>,
    pub tags: Vec<TagSummary>,
    pub stashes: Vec<StashSummary>,
    pub worktrees: Vec<WorktreeSummary>,
    pub submodules: Vec<SubmoduleSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotDelta {
    pub repository_id: String,
    #[serde(default)]
    pub identity_changed: bool,
    pub head_changed: bool,
    pub upstream_changed: bool,
    pub operation_changed: bool,
    pub paths: PathDelta,
    pub remotes_changed: bool,
    pub branches_changed: bool,
    pub tags_changed: bool,
    pub stashes_changed: bool,
    pub worktrees_changed: bool,
    pub submodules_changed: bool,
}

impl SnapshotDelta {
    pub fn has_changes(&self) -> bool {
        self.head_changed
            || self.identity_changed
            || self.upstream_changed
            || self.operation_changed
            || self.paths.has_changes()
            || self.remotes_changed
            || self.branches_changed
            || self.tags_changed
            || self.stashes_changed
            || self.worktrees_changed
            || self.submodules_changed
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotPatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<RepositoryIdentity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head: Option<HeadState>,
    #[serde(
        default,
        deserialize_with = "deserialize_optional_patch_value",
        skip_serializing_if = "Option::is_none"
    )]
    pub head_commit: Option<Option<CommitSummary>>,
    #[serde(
        default,
        deserialize_with = "deserialize_optional_patch_value",
        skip_serializing_if = "Option::is_none"
    )]
    pub upstream: Option<Option<UpstreamState>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paths: Option<PathState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation: Option<OperationState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remotes: Option<Vec<RemoteSummary>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branches: Option<Vec<BranchSummary>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<TagSummary>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stashes: Option<Vec<StashSummary>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktrees: Option<Vec<WorktreeSummary>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub submodules: Option<Vec<SubmoduleSummary>>,
}

impl SnapshotPatch {
    pub fn from_delta(current: &RepositorySnapshot, delta: &SnapshotDelta) -> Self {
        Self {
            identity: delta.identity_changed.then(|| current.identity.clone()),
            head: delta.head_changed.then(|| current.head.clone()),
            head_commit: delta.head_changed.then(|| current.head_commit.clone()),
            upstream: delta.upstream_changed.then(|| current.upstream.clone()),
            paths: delta.paths.has_changes().then(|| current.paths.clone()),
            operation: delta.operation_changed.then(|| current.operation.clone()),
            remotes: delta.remotes_changed.then(|| current.remotes.clone()),
            branches: delta.branches_changed.then(|| current.branches.clone()),
            tags: delta.tags_changed.then(|| current.tags.clone()),
            stashes: delta.stashes_changed.then(|| current.stashes.clone()),
            worktrees: delta.worktrees_changed.then(|| current.worktrees.clone()),
            submodules: delta.submodules_changed.then(|| current.submodules.clone()),
        }
    }
}

fn deserialize_optional_patch_value<'de, D, T>(
    deserializer: D,
) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RefreshDomain {
    Identity,
    Head,
    Upstream,
    Paths,
    Operation,
    Remotes,
    Branches,
    Tags,
    Stashes,
    Worktrees,
    Submodules,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshPlan {
    None,
    Full,
    Domains(BTreeSet<RefreshDomain>),
}

impl RefreshPlan {
    pub fn domains(domains: impl IntoIterator<Item = RefreshDomain>) -> Self {
        let domains = domains.into_iter().collect::<BTreeSet<_>>();
        if domains.is_empty() {
            Self::None
        } else {
            Self::Domains(domains)
        }
    }

    pub fn should_refresh(&self) -> bool {
        !matches!(self, Self::None)
    }

    pub fn combine(self, other: Self) -> Self {
        match (self, other) {
            (Self::Full, _) | (_, Self::Full) => Self::Full,
            (Self::None, plan) | (plan, Self::None) => plan,
            (Self::Domains(mut left), Self::Domains(right)) => {
                left.extend(right);
                Self::Domains(left)
            }
        }
    }

    pub fn domain_set(&self) -> Option<&BTreeSet<RefreshDomain>> {
        match self {
            Self::Domains(domains) => Some(domains),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotRefresh {
    pub snapshot: RepositorySnapshot,
    pub plan: RefreshPlan,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PathDelta {
    pub staged: PathSetDelta,
    pub unstaged: PathSetDelta,
    pub untracked: PathSetDelta,
    pub ignored: PathSetDelta,
    pub conflicted: PathSetDelta,
    pub entries_changed: Vec<String>,
}

impl PathDelta {
    pub fn has_changes(&self) -> bool {
        self.staged.has_changes()
            || self.unstaged.has_changes()
            || self.untracked.has_changes()
            || self.ignored.has_changes()
            || self.conflicted.has_changes()
            || !self.entries_changed.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PathSetDelta {
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

impl PathSetDelta {
    pub fn has_changes(&self) -> bool {
        !self.added.is_empty() || !self.removed.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotOptions {
    #[serde(default)]
    pub include_ignored: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryIdentity {
    pub id: String,
    pub worktree_root: Option<PathBuf>,
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
    pub namespace: Option<String>,
    pub is_bare: bool,
    pub is_empty: bool,
    pub is_shallow: bool,
    pub is_linked_worktree: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeadState {
    pub kind: HeadKind,
    pub name: Option<String>,
    pub branch: Option<String>,
    pub oid: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitSummary {
    pub oid: String,
    pub parent_oids: Vec<String>,
    pub summary: Option<String>,
    pub author_name: Option<String>,
    pub author_email: Option<String>,
    pub time_seconds: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HeadKind {
    Attached,
    Detached,
    Unborn,
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpstreamState {
    pub name: String,
    pub oid: Option<String>,
    pub ahead: usize,
    pub behind: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PathState {
    pub staged: Vec<String>,
    pub unstaged: Vec<String>,
    pub untracked: Vec<String>,
    #[serde(default)]
    pub ignored: Vec<String>,
    pub conflicted: Vec<String>,
    pub conflicts: Vec<ConflictSummary>,
    pub entries: Vec<PathEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictSummary {
    pub path: String,
    pub ancestor: Option<ConflictSide>,
    pub ours: Option<ConflictSide>,
    pub theirs: Option<ConflictSide>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictSide {
    pub path: String,
    pub oid: String,
    pub mode: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PathEntry {
    pub path: String,
    pub staged_old_path: Option<String>,
    pub staged_new_path: Option<String>,
    pub workdir_old_path: Option<String>,
    pub workdir_new_path: Option<String>,
    pub status: PathEntryStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PathEntryStatus {
    pub index_new: bool,
    pub index_modified: bool,
    pub index_deleted: bool,
    pub index_renamed: bool,
    pub index_typechange: bool,
    pub workdir_new: bool,
    pub workdir_modified: bool,
    pub workdir_deleted: bool,
    pub workdir_typechange: bool,
    pub workdir_renamed: bool,
    pub workdir_unreadable: bool,
    pub ignored: bool,
    pub conflicted: bool,
    #[serde(default)]
    pub assume_unchanged: bool,
    #[serde(default)]
    pub skip_worktree: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationState {
    pub kind: OperationKind,
    pub message: Option<String>,
    #[serde(default)]
    pub heads: Vec<OperationHead>,
    #[serde(default)]
    pub bisect: Option<BisectState>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationHead {
    pub role: OperationHeadRole,
    pub oid: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OperationHeadRole {
    Merge,
    Rebase,
    CherryPick,
    Revert,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BisectState {
    pub start_oid: Option<String>,
    pub good_oids: Vec<String>,
    pub bad_oids: Vec<String>,
    pub skipped_oids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OperationKind {
    Clean,
    Merge,
    Revert,
    RevertSequence,
    CherryPick,
    CherryPickSequence,
    Bisect,
    Rebase,
    RebaseInteractive,
    RebaseMerge,
    ApplyMailbox,
    ApplyMailboxOrRebase,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSummary {
    pub name: String,
    pub url: Option<String>,
    pub push_url: Option<String>,
    pub default_branch: Option<String>,
    pub fetch_refspecs: Vec<String>,
    pub push_refspecs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchSummary {
    pub name: String,
    pub kind: BranchKind,
    pub is_head: bool,
    pub oid: Option<String>,
    pub tip_commit: Option<CommitSummary>,
    pub upstream: Option<String>,
    pub upstream_ahead: Option<usize>,
    pub upstream_behind: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BranchKind {
    Local,
    Remote,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagSummary {
    pub name: String,
    pub oid: String,
    pub kind: TagKind,
    pub target_oid: String,
    pub target_kind: Option<GitObjectKind>,
    pub tagger_name: Option<String>,
    pub tagger_email: Option<String>,
    pub tagger_time_seconds: Option<i64>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TagKind {
    Lightweight,
    Annotated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GitObjectKind {
    Commit,
    Tree,
    Blob,
    Tag,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StashSummary {
    pub index: usize,
    pub message: String,
    pub oid: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeSummary {
    pub name: String,
    pub path: PathBuf,
    pub locked: bool,
    pub lock_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmoduleSummary {
    pub name: String,
    pub path: PathBuf,
    pub url: Option<String>,
    pub branch: Option<String>,
    pub head_oid: Option<String>,
    pub index_oid: Option<String>,
    pub workdir_oid: Option<String>,
    pub status: SubmoduleState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmoduleState {
    pub in_head: bool,
    pub in_index: bool,
    pub in_config: bool,
    pub in_workdir: bool,
    pub index_added: bool,
    pub index_deleted: bool,
    pub index_modified: bool,
    pub workdir_uninitialized: bool,
    pub workdir_added: bool,
    pub workdir_deleted: bool,
    pub workdir_modified: bool,
    pub workdir_worktree_modified: bool,
    pub workdir_untracked: bool,
}
