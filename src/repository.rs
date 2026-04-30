use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use git2::{
    BranchType, ErrorCode, IndexEntryExtendedFlag, IndexEntryFlag, ObjectType, Repository,
    RepositoryState, Status, StatusOptions, StatusShow, SubmoduleIgnore, SubmoduleStatus,
    WorktreeLockStatus,
};
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

fn path_delta(previous: &PathState, current: &PathState) -> PathDelta {
    let previous_entries = path_entry_map(&previous.entries);
    let current_entries = path_entry_map(&current.entries);
    let entries_changed = previous_entries
        .keys()
        .chain(current_entries.keys())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter_map(|path| {
            if previous_entries.get(&path) == current_entries.get(&path) {
                None
            } else {
                Some(path)
            }
        })
        .collect();

    PathDelta {
        staged: path_set_delta(&previous.staged, &current.staged),
        unstaged: path_set_delta(&previous.unstaged, &current.unstaged),
        untracked: path_set_delta(&previous.untracked, &current.untracked),
        ignored: path_set_delta(&previous.ignored, &current.ignored),
        conflicted: path_set_delta(&previous.conflicted, &current.conflicted),
        entries_changed,
    }
}

fn path_set_delta(previous: &[String], current: &[String]) -> PathSetDelta {
    let previous = previous.iter().cloned().collect::<BTreeSet<_>>();
    let current = current.iter().cloned().collect::<BTreeSet<_>>();
    PathSetDelta {
        added: current.difference(&previous).cloned().collect(),
        removed: previous.difference(&current).cloned().collect(),
    }
}

fn path_entry_map(entries: &[PathEntry]) -> BTreeMap<String, &PathEntry> {
    entries
        .iter()
        .map(|entry| (entry.path.clone(), entry))
        .collect::<BTreeMap<_, _>>()
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

fn repository_identity(repo: &Repository) -> Result<RepositoryIdentity, SnapshotError> {
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
        is_empty: repo.is_empty()?,
        is_shallow: repo.is_shallow(),
        is_linked_worktree,
    })
}

fn head_state(repo: &Repository) -> Result<HeadState, SnapshotError> {
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

fn head_commit(
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

fn commit_summary(repo: &Repository, oid: git2::Oid) -> Option<CommitSummary> {
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

fn upstream_state(
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

fn path_state(repo: &Repository, options: SnapshotOptions) -> Result<PathState, SnapshotError> {
    if repo.is_bare() {
        return Ok(PathState::default());
    }

    let mut opts = StatusOptions::new();
    opts.show(StatusShow::IndexAndWorkdir)
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(options.include_ignored)
        .renames_head_to_index(true)
        .renames_index_to_workdir(true);

    let statuses = repo.statuses(Some(&mut opts))?;
    let mut staged = BTreeSet::new();
    let mut unstaged = BTreeSet::new();
    let mut untracked = BTreeSet::new();
    let mut ignored = BTreeSet::new();
    let mut conflicted = BTreeSet::new();
    let mut entries = Vec::new();
    let conflicts = conflict_summaries(repo)?;

    for entry in statuses.iter() {
        let status = entry.status();
        let Some(path) = entry.path().map(ToString::to_string) else {
            continue;
        };

        if status.contains(Status::CONFLICTED) {
            conflicted.insert(path.clone());
        }
        if has_staged_status(status) {
            staged.insert(path.clone());
        }
        if status.contains(Status::WT_NEW) {
            untracked.insert(path.clone());
        }
        if status.contains(Status::IGNORED) {
            ignored.insert(path.clone());
        }
        entries.push(PathEntry {
            path: path.clone(),
            staged_old_path: entry
                .head_to_index()
                .and_then(|delta| delta.old_file().path().map(path_to_string)),
            staged_new_path: entry
                .head_to_index()
                .and_then(|delta| delta.new_file().path().map(path_to_string)),
            workdir_old_path: entry
                .index_to_workdir()
                .and_then(|delta| delta.old_file().path().map(path_to_string)),
            workdir_new_path: entry
                .index_to_workdir()
                .and_then(|delta| delta.new_file().path().map(path_to_string)),
            status: path_entry_status(status),
        });
        if has_unstaged_status(status) {
            unstaged.insert(path);
        }
    }
    include_index_flag_entries(repo, &mut entries)?;
    entries.sort_by(|a, b| a.path.cmp(&b.path));

    Ok(PathState {
        staged: staged.into_iter().collect(),
        unstaged: unstaged.into_iter().collect(),
        untracked: untracked.into_iter().collect(),
        ignored: ignored.into_iter().collect(),
        conflicted: conflicted.into_iter().collect(),
        conflicts,
        entries,
    })
}

fn conflict_summaries(repo: &Repository) -> Result<Vec<ConflictSummary>, SnapshotError> {
    let index = repo.index()?;
    let mut conflicts = Vec::new();
    for conflict in index.conflicts()? {
        let conflict = conflict?;
        let ancestor = conflict.ancestor.map(conflict_side);
        let ours = conflict.our.map(conflict_side);
        let theirs = conflict.their.map(conflict_side);
        let Some(path) = ancestor
            .as_ref()
            .or(ours.as_ref())
            .or(theirs.as_ref())
            .map(|side| side.path.clone())
        else {
            continue;
        };

        conflicts.push(ConflictSummary {
            path,
            ancestor,
            ours,
            theirs,
        });
    }
    conflicts.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(conflicts)
}

fn include_index_flag_entries(
    repo: &Repository,
    entries: &mut Vec<PathEntry>,
) -> Result<(), SnapshotError> {
    let index = repo.index()?;
    for entry in index.iter() {
        let assume_unchanged = IndexEntryFlag::from_bits_truncate(entry.flags).is_valid();
        let skip_worktree =
            IndexEntryExtendedFlag::from_bits_truncate(entry.flags_extended).is_skip_worktree();
        if !assume_unchanged && !skip_worktree {
            continue;
        }

        let path = String::from_utf8_lossy(&entry.path).into_owned();
        if let Some(existing) = entries.iter_mut().find(|existing| existing.path == path) {
            existing.status.assume_unchanged = assume_unchanged;
            existing.status.skip_worktree = skip_worktree;
        } else {
            entries.push(PathEntry {
                path,
                staged_old_path: None,
                staged_new_path: None,
                workdir_old_path: None,
                workdir_new_path: None,
                status: PathEntryStatus {
                    assume_unchanged,
                    skip_worktree,
                    ..PathEntryStatus::default()
                },
            });
        }
    }
    Ok(())
}

fn conflict_side(entry: git2::IndexEntry) -> ConflictSide {
    ConflictSide {
        path: String::from_utf8_lossy(&entry.path).into_owned(),
        oid: entry.id.to_string(),
        mode: entry.mode,
    }
}

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn path_entry_status(status: Status) -> PathEntryStatus {
    PathEntryStatus {
        index_new: status.contains(Status::INDEX_NEW),
        index_modified: status.contains(Status::INDEX_MODIFIED),
        index_deleted: status.contains(Status::INDEX_DELETED),
        index_renamed: status.contains(Status::INDEX_RENAMED),
        index_typechange: status.contains(Status::INDEX_TYPECHANGE),
        workdir_new: status.contains(Status::WT_NEW),
        workdir_modified: status.contains(Status::WT_MODIFIED),
        workdir_deleted: status.contains(Status::WT_DELETED),
        workdir_typechange: status.contains(Status::WT_TYPECHANGE),
        workdir_renamed: status.contains(Status::WT_RENAMED),
        workdir_unreadable: status.contains(Status::WT_UNREADABLE),
        ignored: status.contains(Status::IGNORED),
        conflicted: status.contains(Status::CONFLICTED),
        ..PathEntryStatus::default()
    }
}

fn has_staged_status(status: Status) -> bool {
    status.intersects(
        Status::INDEX_NEW
            | Status::INDEX_MODIFIED
            | Status::INDEX_DELETED
            | Status::INDEX_RENAMED
            | Status::INDEX_TYPECHANGE,
    )
}

fn has_unstaged_status(status: Status) -> bool {
    status.intersects(
        Status::WT_MODIFIED
            | Status::WT_DELETED
            | Status::WT_TYPECHANGE
            | Status::WT_RENAMED
            | Status::WT_UNREADABLE,
    )
}

fn operation_state(repo: &Repository) -> OperationState {
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

fn operation_heads(repo: &Repository) -> Vec<OperationHead> {
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

fn read_operation_head_file(
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

fn bisect_state(repo: &Repository) -> BisectState {
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
        let Some(name) = reference.name() else {
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

fn read_operation_oid_file(repo: &Repository, relative_path: &str) -> Option<String> {
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

fn remotes(repo: &Repository) -> Result<Vec<RemoteSummary>, SnapshotError> {
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

fn remote_default_branch(repo: &Repository, remote_name: &str) -> Option<String> {
    let reference = repo
        .find_reference(&format!("refs/remotes/{remote_name}/HEAD"))
        .ok()?;
    let target = reference.symbolic_target()?;
    target
        .strip_prefix(&format!("refs/remotes/{remote_name}/"))
        .map(ToString::to_string)
}

fn branches(repo: &Repository) -> Result<Vec<BranchSummary>, SnapshotError> {
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

fn branch_upstream_counts(
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

fn tags(repo: &Repository) -> Result<Vec<TagSummary>, SnapshotError> {
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

fn git_object_kind(kind: ObjectType) -> Option<GitObjectKind> {
    match kind {
        ObjectType::Commit => Some(GitObjectKind::Commit),
        ObjectType::Tree => Some(GitObjectKind::Tree),
        ObjectType::Blob => Some(GitObjectKind::Blob),
        ObjectType::Tag => Some(GitObjectKind::Tag),
        ObjectType::Any => None,
    }
}

fn stashes(repo: &mut Repository) -> Result<Vec<StashSummary>, SnapshotError> {
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

fn worktrees(repo: &Repository) -> Result<Vec<WorktreeSummary>, SnapshotError> {
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

fn submodules(repo: &Repository) -> Result<Vec<SubmoduleSummary>, SnapshotError> {
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
            url: submodule.url().map(ToString::to_string),
            branch: submodule.branch().map(ToString::to_string),
            head_oid: submodule.head_id().map(|oid| oid.to_string()),
            index_oid: submodule.index_id().map(|oid| oid.to_string()),
            workdir_oid: submodule.workdir_id().map(|oid| oid.to_string()),
            status: submodule_state(status),
        });
    }
    summaries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(summaries)
}

fn submodule_state(status: SubmoduleStatus) -> SubmoduleState {
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

fn stable_path_id(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process::Command;

    use super::*;
    use tempfile::TempDir;

    #[test]
    fn rejects_non_repository_paths() {
        let temp = TempDir::new().unwrap();

        let error = snapshot_repository(temp.path()).unwrap_err();

        assert!(matches!(error, SnapshotError::NotRepository { .. }));
    }

    #[test]
    fn snapshots_empty_repository_identity() {
        let repo = TestRepo::new();

        let snapshot = snapshot_repository(repo.path()).unwrap();

        assert!(snapshot.identity.is_empty);
        assert!(!snapshot.identity.is_bare);
        assert!(!snapshot.identity.is_shallow);
        assert_eq!(snapshot.identity.namespace, None);
        assert_eq!(snapshot.head.kind, HeadKind::Unborn);
        assert_eq!(snapshot.head.name, None);
        assert_eq!(snapshot.head.branch, None);
    }

    #[test]
    fn snapshots_clean_repository_identity_and_head() {
        let repo = TestRepo::new();
        repo.write("README.md", "hello\n");
        repo.git(["add", "README.md"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git(["tag", "v0.1.0"]);
        repo.git(["tag", "-a", "v0.2.0", "-m", "release v0.2.0"]);

        let snapshot = snapshot_repository(repo.path()).unwrap();

        assert_eq!(
            snapshot.identity.worktree_root.as_deref(),
            Some(repo.path())
        );
        assert!(!snapshot.identity.is_empty);
        assert!(!snapshot.identity.is_shallow);
        assert_eq!(snapshot.identity.namespace, None);
        assert_eq!(snapshot.head.kind, HeadKind::Attached);
        assert_eq!(snapshot.head.name.as_deref(), Some("refs/heads/main"));
        assert!(snapshot.head.branch.is_some());
        assert!(snapshot.head.oid.is_some());
        assert_eq!(
            snapshot
                .head_commit
                .as_ref()
                .and_then(|commit| commit.summary.as_deref()),
            Some("initial")
        );
        assert_eq!(
            snapshot
                .head_commit
                .as_ref()
                .and_then(|commit| commit.author_email.as_deref()),
            Some("tester@example.com")
        );
        assert_eq!(snapshot.operation.kind, OperationKind::Clean);
        assert_eq!(snapshot.operation.message, None);
        assert!(snapshot.operation.heads.is_empty());
        assert!(snapshot.paths.staged.is_empty());
        assert!(snapshot.paths.unstaged.is_empty());
        assert!(snapshot.paths.untracked.is_empty());
        let head_branch = snapshot
            .branches
            .iter()
            .find(|branch| branch.kind == BranchKind::Local && branch.is_head)
            .unwrap();
        assert_eq!(
            head_branch
                .tip_commit
                .as_ref()
                .and_then(|commit| commit.summary.as_deref()),
            Some("initial")
        );
        let lightweight_tag = snapshot
            .tags
            .iter()
            .find(|tag| tag.name == "v0.1.0")
            .unwrap();
        assert_eq!(lightweight_tag.kind, TagKind::Lightweight);
        assert_eq!(lightweight_tag.oid, lightweight_tag.target_oid);
        assert_eq!(lightweight_tag.target_kind, Some(GitObjectKind::Commit));
        assert_eq!(lightweight_tag.message, None);
        let annotated_tag = snapshot
            .tags
            .iter()
            .find(|tag| tag.name == "v0.2.0")
            .unwrap();
        assert_eq!(annotated_tag.kind, TagKind::Annotated);
        assert_ne!(annotated_tag.oid, annotated_tag.target_oid);
        assert_eq!(annotated_tag.target_kind, Some(GitObjectKind::Commit));
        assert_eq!(
            annotated_tag.tagger_email.as_deref(),
            Some("tester@example.com")
        );
        assert_eq!(annotated_tag.message.as_deref(), Some("release v0.2.0\n"));
    }

    #[test]
    fn snapshots_detached_head_oid() {
        let repo = TestRepo::new();
        repo.write("README.md", "hello\n");
        repo.git(["add", "README.md"]);
        repo.git(["commit", "-m", "initial"]);
        let head_oid = repo.git_stdout(["rev-parse", "HEAD"]);
        repo.git(["checkout", "--detach", &head_oid]);

        let snapshot = snapshot_repository(repo.path()).unwrap();

        assert_eq!(snapshot.head.kind, HeadKind::Detached);
        assert_eq!(snapshot.head.branch, None);
        assert_eq!(snapshot.head.oid.as_deref(), Some(head_oid.as_str()));
        assert_eq!(
            snapshot.head_commit.and_then(|commit| commit.summary),
            Some("initial".to_string())
        );
    }

    #[test]
    fn snapshots_round_trip_through_json() {
        let repo = TestRepo::new();
        repo.write("README.md", "hello\n");
        repo.git(["add", "README.md"]);
        repo.git(["commit", "-m", "initial"]);
        let snapshot = snapshot_repository(repo.path()).unwrap();

        let serialized = serde_json::to_string(&snapshot).unwrap();
        let deserialized: RepositorySnapshot = serde_json::from_str(&serialized).unwrap();

        assert_eq!(deserialized, snapshot);
    }

    #[test]
    fn deserializes_operation_state_without_heads() {
        let operation: OperationState =
            serde_json::from_str(r#"{"kind":"clean","message":null}"#).unwrap();

        assert_eq!(operation.kind, OperationKind::Clean);
        assert_eq!(operation.message, None);
        assert!(operation.heads.is_empty());
    }

    #[test]
    fn separates_staged_unstaged_and_untracked_paths() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);

        repo.write("tracked.txt", "changed\n");
        repo.write("staged.txt", "staged\n");
        repo.git(["add", "staged.txt"]);
        repo.write("untracked.txt", "untracked\n");

        let snapshot = snapshot_repository(repo.path()).unwrap();

        assert_eq!(snapshot.paths.staged, vec!["staged.txt"]);
        assert_eq!(snapshot.paths.unstaged, vec!["tracked.txt"]);
        assert_eq!(snapshot.paths.untracked, vec!["untracked.txt"]);
        assert!(snapshot.paths.conflicted.is_empty());
        let staged = snapshot
            .paths
            .entries
            .iter()
            .find(|entry| entry.path == "staged.txt")
            .unwrap();
        assert!(staged.status.index_new);
        assert_eq!(staged.staged_new_path.as_deref(), Some("staged.txt"));
        let unstaged = snapshot
            .paths
            .entries
            .iter()
            .find(|entry| entry.path == "tracked.txt")
            .unwrap();
        assert!(unstaged.status.workdir_modified);
        assert_eq!(unstaged.workdir_new_path.as_deref(), Some("tracked.txt"));
        let untracked = snapshot
            .paths
            .entries
            .iter()
            .find(|entry| entry.path == "untracked.txt")
            .unwrap();
        assert!(untracked.status.workdir_new);
    }

    #[test]
    fn omits_ignored_paths_by_default_and_includes_when_requested() {
        let repo = TestRepo::new();
        repo.write(".gitignore", "ignored.txt\n");
        repo.git(["add", ".gitignore"]);
        repo.git(["commit", "-m", "ignore rules"]);
        repo.write("ignored.txt", "ignored\n");

        let default_snapshot = snapshot_repository(repo.path()).unwrap();

        assert!(default_snapshot.paths.ignored.is_empty());
        assert!(
            default_snapshot
                .paths
                .entries
                .iter()
                .all(|entry| entry.path != "ignored.txt")
        );

        let snapshot = snapshot_repository_with_options(
            repo.path(),
            SnapshotOptions {
                include_ignored: true,
            },
        )
        .unwrap();

        assert_eq!(snapshot.paths.ignored, vec!["ignored.txt"]);
        let ignored = snapshot
            .paths
            .entries
            .iter()
            .find(|entry| entry.path == "ignored.txt")
            .unwrap();
        assert!(ignored.status.ignored);
    }

    #[test]
    fn deserializes_path_state_without_ignored_paths() {
        let path_state: PathState = serde_json::from_str(
            r#"{"staged":[],"unstaged":[],"untracked":[],"conflicted":[],"conflicts":[],"entries":[]}"#,
        )
        .unwrap();

        assert!(path_state.ignored.is_empty());
    }

    #[test]
    fn computes_coarse_snapshot_deltas() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        let previous = snapshot_repository(repo.path()).unwrap();

        repo.write("tracked.txt", "changed\n");
        repo.write("new.txt", "new\n");
        let current = snapshot_repository(repo.path()).unwrap();

        let delta = snapshot_delta(&previous, &current);

        assert!(!delta.head_changed);
        assert!(!delta.operation_changed);
        assert_eq!(delta.paths.unstaged.added, vec!["tracked.txt"]);
        assert_eq!(delta.paths.untracked.added, vec!["new.txt"]);
        assert!(
            delta
                .paths
                .entries_changed
                .contains(&"tracked.txt".to_string())
        );
        assert!(delta.paths.entries_changed.contains(&"new.txt".to_string()));
    }

    #[test]
    fn delta_reports_identity_changes() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        let previous = snapshot_repository(repo.path()).unwrap();
        let mut current = previous.clone();
        current.identity.namespace = Some("namespace".to_string());

        let delta = snapshot_delta(&previous, &current);
        let patch = SnapshotPatch::from_delta(&current, &delta);

        assert!(delta.identity_changed);
        assert_eq!(
            patch.identity.and_then(|identity| identity.namespace),
            Some("namespace".to_string())
        );
    }

    #[test]
    fn includes_commit_parent_oids() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        let first_commit = repo.git_stdout(["rev-parse", "HEAD"]);
        repo.write("tracked.txt", "second\n");
        repo.git(["commit", "-am", "second"]);

        let snapshot = snapshot_repository(repo.path()).unwrap();

        let head_commit = snapshot.head_commit.as_ref().unwrap();
        assert_eq!(head_commit.summary.as_deref(), Some("second"));
        assert_eq!(head_commit.parent_oids, vec![first_commit]);
        let head_branch = snapshot
            .branches
            .iter()
            .find(|branch| branch.is_head)
            .unwrap();
        assert_eq!(
            head_branch
                .tip_commit
                .as_ref()
                .map(|commit| &commit.parent_oids),
            Some(&head_commit.parent_oids)
        );
    }

    #[test]
    fn detects_merge_operation_and_conflicted_paths() {
        let repo = TestRepo::new();
        repo.write("conflict.txt", "base\n");
        repo.git(["add", "conflict.txt"]);
        repo.git(["commit", "-m", "initial"]);
        let base_branch = repo.current_branch();

        repo.git(["checkout", "-b", "side"]);
        repo.write("conflict.txt", "side\n");
        repo.git(["commit", "-am", "side"]);

        repo.git(["checkout", &base_branch]);
        repo.write("conflict.txt", "main\n");
        repo.git(["commit", "-am", "main"]);
        repo.git_expect_failure(["merge", "side"]);

        let snapshot = snapshot_repository(repo.path()).unwrap();

        assert_eq!(snapshot.operation.kind, OperationKind::Merge);
        assert!(snapshot.operation.message.is_some());
        let merge_head = snapshot
            .operation
            .heads
            .iter()
            .find(|head| head.role == OperationHeadRole::Merge)
            .unwrap();
        assert_eq!(merge_head.oid, repo.git_stdout(["rev-parse", "side"]));
        assert_eq!(snapshot.paths.conflicted, vec!["conflict.txt"]);
        assert_eq!(snapshot.paths.conflicts.len(), 1);
        let conflict = &snapshot.paths.conflicts[0];
        assert_eq!(conflict.path, "conflict.txt");
        let ancestor = conflict.ancestor.as_ref().unwrap();
        let ours = conflict.ours.as_ref().unwrap();
        let theirs = conflict.theirs.as_ref().unwrap();
        assert_eq!(ancestor.path, "conflict.txt");
        assert_eq!(ours.path, "conflict.txt");
        assert_eq!(theirs.path, "conflict.txt");
        assert_eq!(ancestor.mode, 0o100644);
        assert_eq!(ours.mode, 0o100644);
        assert_eq!(theirs.mode, 0o100644);
        assert_ne!(ancestor.oid, ours.oid);
        assert_ne!(ours.oid, theirs.oid);
    }

    #[test]
    fn detects_rebase_operation_and_head_oid() {
        let repo = TestRepo::new();
        repo.write("conflict.txt", "base\n");
        repo.git(["add", "conflict.txt"]);
        repo.git(["commit", "-m", "initial"]);
        let base_branch = repo.current_branch();

        repo.git(["checkout", "-b", "side"]);
        repo.write("conflict.txt", "side\n");
        repo.git(["commit", "-am", "side"]);
        let rebased_oid = repo.git_stdout(["rev-parse", "HEAD"]);

        repo.git(["checkout", &base_branch]);
        repo.write("conflict.txt", "main\n");
        repo.git(["commit", "-am", "main"]);

        repo.git(["checkout", "side"]);
        repo.git_expect_failure(["rebase", &base_branch]);

        let snapshot = snapshot_repository(repo.path()).unwrap();

        assert!(matches!(
            snapshot.operation.kind,
            OperationKind::Rebase | OperationKind::RebaseInteractive | OperationKind::RebaseMerge
        ));
        let rebase_head = snapshot
            .operation
            .heads
            .iter()
            .find(|head| head.role == OperationHeadRole::Rebase)
            .unwrap();
        assert_eq!(rebase_head.oid, rebased_oid);
        assert_eq!(snapshot.paths.conflicted, vec!["conflict.txt"]);
    }

    #[test]
    fn detects_cherry_pick_operation_and_head_oid() {
        let repo = TestRepo::new();
        repo.write("conflict.txt", "base\n");
        repo.git(["add", "conflict.txt"]);
        repo.git(["commit", "-m", "initial"]);
        let base_branch = repo.current_branch();

        repo.git(["checkout", "-b", "side"]);
        repo.write("conflict.txt", "side\n");
        repo.git(["commit", "-am", "side"]);
        let picked_oid = repo.git_stdout(["rev-parse", "HEAD"]);

        repo.git(["checkout", &base_branch]);
        repo.write("conflict.txt", "main\n");
        repo.git(["commit", "-am", "main"]);
        repo.git_expect_failure(["cherry-pick", &picked_oid]);

        let snapshot = snapshot_repository(repo.path()).unwrap();

        assert!(matches!(
            snapshot.operation.kind,
            OperationKind::CherryPick | OperationKind::CherryPickSequence
        ));
        let cherry_pick_head = snapshot
            .operation
            .heads
            .iter()
            .find(|head| head.role == OperationHeadRole::CherryPick)
            .unwrap();
        assert_eq!(cherry_pick_head.oid, picked_oid);
        assert_eq!(snapshot.paths.conflicted, vec!["conflict.txt"]);
    }

    #[test]
    fn detects_revert_operation_and_head_oid() {
        let repo = TestRepo::new();
        repo.write("conflict.txt", "base\n");
        repo.git(["add", "conflict.txt"]);
        repo.git(["commit", "-m", "initial"]);

        repo.write("conflict.txt", "target\n");
        repo.git(["commit", "-am", "target"]);
        let reverted_oid = repo.git_stdout(["rev-parse", "HEAD"]);

        repo.write("conflict.txt", "current\n");
        repo.git(["commit", "-am", "current"]);
        repo.git_expect_failure(["revert", &reverted_oid]);

        let snapshot = snapshot_repository(repo.path()).unwrap();

        assert!(matches!(
            snapshot.operation.kind,
            OperationKind::Revert | OperationKind::RevertSequence
        ));
        let revert_head = snapshot
            .operation
            .heads
            .iter()
            .find(|head| head.role == OperationHeadRole::Revert)
            .unwrap();
        assert_eq!(revert_head.oid, reverted_oid);
        assert_eq!(snapshot.paths.conflicted, vec!["conflict.txt"]);
    }

    #[test]
    fn detects_bisect_operation_and_refs() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "good\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "good"]);
        let good_oid = repo.git_stdout(["rev-parse", "HEAD"]);

        repo.write("tracked.txt", "middle\n");
        repo.git(["commit", "-am", "middle"]);

        repo.write("tracked.txt", "bad\n");
        repo.git(["commit", "-am", "bad"]);
        let bad_oid = repo.git_stdout(["rev-parse", "HEAD"]);

        repo.git(["bisect", "start", &bad_oid, &good_oid]);

        let snapshot = snapshot_repository(repo.path()).unwrap();

        assert_eq!(snapshot.operation.kind, OperationKind::Bisect);
        assert!(snapshot.operation.heads.is_empty());
        let bisect = snapshot.operation.bisect.as_ref().unwrap();
        assert_eq!(bisect.good_oids, vec![good_oid]);
        assert_eq!(bisect.bad_oids, vec![bad_oid]);
        assert!(bisect.skipped_oids.is_empty());
    }

    #[test]
    fn includes_remote_and_upstream_summary() {
        let remote = TestRepo::new();
        remote.write("README.md", "hello\n");
        remote.git(["add", "README.md"]);
        remote.git(["commit", "-m", "initial"]);

        let local = TestRepo::clone_from(remote.path());
        local.git(["config", "branch.main.remote", "origin"]);
        local.git(["config", "branch.main.merge", "refs/heads/main"]);

        let snapshot = snapshot_repository(local.path()).unwrap();

        let origin = snapshot
            .remotes
            .iter()
            .find(|remote| remote.name == "origin")
            .unwrap();
        assert_eq!(origin.default_branch.as_deref(), Some("main"));
        assert!(
            origin
                .fetch_refspecs
                .iter()
                .any(|refspec| refspec == "+refs/heads/*:refs/remotes/origin/*")
        );
        assert!(origin.push_refspecs.is_empty());
        assert!(snapshot.upstream.is_some());
        let head = snapshot
            .branches
            .iter()
            .find(|branch| branch.is_head)
            .unwrap();
        assert_eq!(head.upstream.as_deref(), Some("origin/main"));
        assert_eq!(head.upstream_ahead, Some(0));
        assert_eq!(head.upstream_behind, Some(0));
    }

    #[test]
    fn includes_stash_summaries() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.write("tracked.txt", "stashed\n");
        repo.git(["stash", "push", "-m", "save work"]);

        let snapshot = snapshot_repository(repo.path()).unwrap();

        assert_eq!(snapshot.stashes.len(), 1);
        assert_eq!(snapshot.stashes[0].index, 0);
        assert!(snapshot.stashes[0].message.contains("save work"));
        assert!(!snapshot.stashes[0].oid.is_empty());
    }

    #[test]
    fn includes_linked_worktree_summaries() {
        let repo = TestRepo::new();
        repo.write("README.md", "hello\n");
        repo.git(["add", "README.md"]);
        repo.git(["commit", "-m", "initial"]);
        let linked_parent = TempDir::new().unwrap();
        let linked_path = linked_parent.path().join("linked");
        let linked_path = linked_path.to_str().unwrap();
        repo.git(["worktree", "add", "-b", "linked", linked_path]);

        let snapshot = snapshot_repository(repo.path()).unwrap();

        assert_eq!(snapshot.worktrees.len(), 1);
        assert_eq!(snapshot.worktrees[0].name, "linked");
        assert_eq!(snapshot.worktrees[0].path, PathBuf::from(linked_path));
        assert!(!snapshot.worktrees[0].locked);
        assert_eq!(snapshot.worktrees[0].lock_reason, None);
    }

    #[test]
    fn includes_submodule_summaries() {
        let submodule_repo = TestRepo::new();
        submodule_repo.write("README.md", "submodule\n");
        submodule_repo.git(["add", "README.md"]);
        submodule_repo.git(["commit", "-m", "submodule initial"]);
        let submodule_url = submodule_repo.path().to_str().unwrap();

        let repo = TestRepo::new();
        repo.write("README.md", "super\n");
        repo.git(["add", "README.md"]);
        repo.git(["commit", "-m", "super initial"]);
        repo.git_allow_file_protocol(["submodule", "add", submodule_url, "deps/sub"]);
        repo.git(["commit", "-am", "add submodule"]);

        let snapshot = snapshot_repository(repo.path()).unwrap();

        assert_eq!(snapshot.submodules.len(), 1);
        let submodule = &snapshot.submodules[0];
        assert_eq!(submodule.name, "deps/sub");
        assert_eq!(submodule.path, PathBuf::from("deps/sub"));
        assert_eq!(submodule.url.as_deref(), Some(submodule_url));
        assert!(submodule.head_oid.is_some());
        assert!(submodule.index_oid.is_some());
        assert!(submodule.workdir_oid.is_some());
        assert!(submodule.status.in_head);
        assert!(submodule.status.in_index);
        assert!(submodule.status.in_config);
        assert!(submodule.status.in_workdir);
    }

    #[test]
    fn targeted_path_refresh_updates_paths_without_rebuilding_unrelated_domains() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        let baseline = snapshot_repository(repo.path()).unwrap();

        repo.write("tracked.txt", "changed\n");
        let refresh = refresh_repository_with_plan(
            repo.path(),
            Some(&baseline),
            &RefreshPlan::domains([RefreshDomain::Paths]),
            SnapshotOptions::default(),
        )
        .unwrap();

        assert_eq!(refresh.plan, RefreshPlan::domains([RefreshDomain::Paths]));
        assert_eq!(refresh.snapshot.paths.unstaged, vec!["tracked.txt"]);
        assert_eq!(refresh.snapshot.head, baseline.head);
        assert_eq!(refresh.snapshot.branches, baseline.branches);
        assert_eq!(refresh.snapshot.remotes, baseline.remotes);
    }

    #[test]
    fn targeted_ref_refresh_updates_head_and_branch_state() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "main\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git(["switch", "-c", "side"]);
        repo.write("tracked.txt", "side\n");
        repo.git(["commit", "-am", "side"]);
        repo.git(["switch", "main"]);
        let baseline = snapshot_repository(repo.path()).unwrap();

        repo.git(["switch", "side"]);
        let refresh = refresh_repository_with_plan(
            repo.path(),
            Some(&baseline),
            &RefreshPlan::domains([
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths,
            ]),
            SnapshotOptions::default(),
        )
        .unwrap();

        assert_eq!(refresh.snapshot.head.branch.as_deref(), Some("side"));
        assert_ne!(refresh.snapshot.head, baseline.head);
        assert_ne!(refresh.snapshot.branches, baseline.branches);
        assert!(refresh.snapshot.paths.unstaged.is_empty());
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

        fn current_branch(&self) -> String {
            self.git_stdout(["symbolic-ref", "--short", "HEAD"])
        }

        fn git<const N: usize>(&self, args: [&str; N]) {
            let output = self.git_output(args);
            assert_git_success(output);
        }

        fn git_allow_file_protocol<const N: usize>(&self, args: [&str; N]) {
            let output = Command::new("git")
                .arg("-c")
                .arg("protocol.file.allow=always")
                .args(args)
                .current_dir(self.path())
                .output()
                .unwrap();
            assert_git_success(output);
        }

        fn git_expect_failure<const N: usize>(&self, args: [&str; N]) {
            let output = self.git_output(args);
            assert!(
                !output.status.success(),
                "git command unexpectedly succeeded\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }

        fn git_output<const N: usize>(&self, args: [&str; N]) -> std::process::Output {
            Command::new("git")
                .args(args)
                .current_dir(self.path())
                .output()
                .unwrap()
        }

        fn git_stdout<const N: usize>(&self, args: [&str; N]) -> String {
            let output = self.git_output(args);
            assert_git_success(output)
        }
    }

    fn assert_git_success(output: std::process::Output) -> String {
        assert!(
            output.status.success(),
            "git command failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }
}
