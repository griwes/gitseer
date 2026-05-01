use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use git2::{IndexEntryExtendedFlag, IndexEntryFlag, Repository, Status, StatusOptions, StatusShow};

use super::{
    ConflictSide, ConflictSummary, PathDelta, PathEntry, PathEntryStatus, PathSetDelta, PathState,
    SnapshotError, SnapshotOptions,
};

pub(super) fn path_delta(previous: &PathState, current: &PathState) -> PathDelta {
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

pub(super) fn path_set_delta(previous: &[String], current: &[String]) -> PathSetDelta {
    let previous = previous.iter().cloned().collect::<BTreeSet<_>>();
    let current = current.iter().cloned().collect::<BTreeSet<_>>();
    PathSetDelta {
        added: current.difference(&previous).cloned().collect(),
        removed: previous.difference(&current).cloned().collect(),
    }
}

pub(super) fn path_entry_map(entries: &[PathEntry]) -> BTreeMap<String, &PathEntry> {
    entries
        .iter()
        .map(|entry| (entry.path.clone(), entry))
        .collect::<BTreeMap<_, _>>()
}

pub(super) fn path_state(
    repo: &Repository,
    options: SnapshotOptions,
) -> Result<PathState, SnapshotError> {
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

pub(super) fn conflict_summaries(repo: &Repository) -> Result<Vec<ConflictSummary>, SnapshotError> {
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

pub(super) fn include_index_flag_entries(
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

pub(super) fn conflict_side(entry: git2::IndexEntry) -> ConflictSide {
    ConflictSide {
        path: String::from_utf8_lossy(&entry.path).into_owned(),
        oid: entry.id.to_string(),
        mode: entry.mode,
    }
}

pub(super) fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

pub(super) fn path_entry_status(status: Status) -> PathEntryStatus {
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

pub(super) fn has_staged_status(status: Status) -> bool {
    status.intersects(
        Status::INDEX_NEW
            | Status::INDEX_MODIFIED
            | Status::INDEX_DELETED
            | Status::INDEX_RENAMED
            | Status::INDEX_TYPECHANGE,
    )
}

pub(super) fn has_unstaged_status(status: Status) -> bool {
    status.intersects(
        Status::WT_MODIFIED
            | Status::WT_DELETED
            | Status::WT_TYPECHANGE
            | Status::WT_RENAMED
            | Status::WT_UNREADABLE,
    )
}
