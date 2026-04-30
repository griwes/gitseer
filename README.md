# Gitseer

`gitseer` is a reusable, single-repository Git state worker.

It is the Rust backing process for `stratum.nvim`, but it is intentionally not
Neovim-specific. A supervising client spawns one `gitseer` process per Git
repository, passes the repository path at process start, and communicates over a
small JSON-RPC 2.0 protocol on stdio.

## Scope

Gitseer owns:

- repository identity and validation
- live repository snapshots backed by `libgit2`
- filesystem watching, Git ignore-aware event filtering, and refresh coalescing
- JSON-RPC request/notification types for repository state

Current snapshots include:

- repository identity, namespace/empty/shallow flags, HEAD state with full ref
  names where available, and HEAD commit summary with parent commit ids
- upstream ahead/behind state for the active branch and per-branch upstream counts
- staged, unstaged, untracked, ignored-on-request, and conflicted path sets plus
  per-path status entries and conflict-stage metadata
- current repository operation state, such as merge or rebase, with operation
  message text, operation head OIDs, and bisect refs when present
- remotes with refspec/default-branch state, branches with tip commit summaries, lightweight and annotated tags, stash summaries, linked worktree summaries, and submodules
- an initial full snapshot for subscribed clients, then versioned coarse deltas
  as the normal steady-state update stream

## Stream Contract

`gitseer/subscribe` and resync/error-recovery paths may send full
`gitseer/snapshot` notifications. `gitseer/getSnapshot` and explicit refreshes
may return full snapshots in their responses. Ordinary watched repository
updates should send `gitseer/delta` notifications, not a full snapshot after
every delta.

Deltas are the primary steady-state communication shape. They must carry enough
monotonic version information for a client to detect missed, duplicate, or
out-of-order updates and request a fresh snapshot. Clients should be able to
maintain local state by applying deltas without reparsing a complete repository
snapshot on every file event.

## Refresh Contract

Gitseer should not re-query every libgit2-backed state section for every
repository change. Watcher events should be classified by the Git control files
or worktree paths that changed, then mapped to targeted state-domain refreshes
such as path status, index, head, refs, upstream, operation state, remotes,
stashes, worktrees, submodules, or ignore rules.

Recursive worktree watching should remain viable for large repositories. Ignored
worktree churn must be filtered through Git ignore semantics before refresh work
is scheduled when ignored-path reporting is disabled. Ignore-rule files remain
watched inputs: changing `.gitignore`, nested `.gitignore`, `.git/info/exclude`,
or configured excludes should refresh ignore classification and affected path
state.

Gitseer does not own:

- multi-repository service mode
- worktree creation/switching/pruning
- Git UI flows
- mutation APIs in the MVP

## Development

```sh
cargo fmt --check
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```
