# Gitseer

`gitseer` is a reusable, single-repository Git state worker.

It is the Rust backing process for `stratum.nvim`, but it is intentionally not
Neovim-specific. A supervising client spawns one `gitseer` process per Git
repository, passes the repository path at process start, and communicates over a
small JSON-RPC 2.0 protocol on stdio.

## Requirements

- Rust 1.98 or newer to build from source
- Git and the native dependencies required by the `git2` crate on Linux

Linux is the primary supported and CI-tested platform. Gitseer is in early
development and currently publishes from `main` without a stable release tag.

## Nightly binary

The rolling `nightly` prerelease publishes a Linux amd64 executable named
`gitseer-linux-amd64` and a matching `gitseer-linux-amd64.sha256` checksum. The
nightly tag is rebuilt from the current `main` branch once per day and can also
be refreshed manually through GitHub Actions. It is a development channel, not
a stable compatibility promise.

## Build and use

```sh
cargo build --locked --release
target/release/gitseer capabilities
target/release/gitseer validate --repo /path/to/repository
target/release/gitseer snapshot --repo /path/to/repository
target/release/gitseer serve --repo /path/to/repository
```

`serve` owns exactly one repository and reads one JSON-RPC request object per
stdin line. It writes responses and notifications as one JSON object per stdout
line. Use `gitseer --help` and `gitseer <command> --help` for CLI details.

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

The stdio transport accepts one JSON-RPC 2.0 request object per line. JSON-RPC
batch arrays are not supported and receive an `Invalid Request` response.

`gitseer/subscribe` and resync/error-recovery paths may send full
`gitseer/snapshot` notifications. `gitseer/getSnapshot` and explicit refreshes
may return full snapshots in their responses. Ordinary watched repository
updates produce `gitseer/delta` notifications. Full snapshots are reserved for
the explicit and recovery paths, including watcher overflow or rescan recovery.

Deltas are the primary steady-state communication shape. They carry monotonic
version information so a client can detect missed, duplicate, or out-of-order
updates and request a fresh snapshot. Clients maintain local state by applying
deltas without reparsing a complete repository snapshot on every file event.

## Refresh Contract

Gitseer classifies watcher events by the Git control files or worktree paths
that changed, then maps them to targeted state-domain refreshes such as path
status, index, head, refs, upstream, operation state, remotes, stashes,
worktrees, submodules, or ignore rules. Unaffected libgit2-backed state sections
are retained from the previous snapshot.

Worktree coverage uses nonrecursive watches over the relevant directory tree,
including watches added for newly created directories. When ignored-path
reporting is disabled, ignored directories are omitted from that watch set and
ignored worktree churn is filtered through Git ignore semantics. Ignore-rule
files remain watched inputs: changing `.gitignore`, nested `.gitignore`,
`.git/info/exclude`, or configured excludes refreshes ignore classification and
affected path state.

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

The complete local gate is `scripts/ci/run.sh`.

## License

Apache-2.0. See [`LICENSE`](LICENSE). Dependency license policy is enforced by
`scripts/ci/check-licenses.py`.
