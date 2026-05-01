use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use clap::{Parser, Subcommand};
use gitseer::{
    Capabilities, Config, ProcessState, RefreshPlan, RepositoryWatcher, ServerMessage,
    goodbye_message, handle_request, refresh_repository_with_plan, snapshot_repository,
    snapshot_update_messages,
};
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

const WATCHER_STARTUP_POLL_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Print machine-readable process capabilities.
    Capabilities,
    /// Print one repository snapshot and exit.
    Snapshot {
        /// Repository or worktree path this process will read.
        #[arg(long)]
        repo: PathBuf,
    },
    /// Serve JSON-RPC over stdio for one repository.
    Serve {
        /// Repository or worktree path this process will own.
        #[arg(long)]
        repo: PathBuf,
    },
    /// Validate startup configuration for a single repository worker.
    Validate {
        /// Repository or worktree path this process will own.
        #[arg(long)]
        repo: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Command::Capabilities) => {
            print_json(&Capabilities::current()).await?;
        }
        Some(Command::Snapshot { repo }) => {
            let snapshot = snapshot_repository(repo)?;
            print_json(&snapshot).await?;
        }
        Some(Command::Serve { repo }) => {
            serve(repo).await?;
        }
        Some(Command::Validate { repo }) => {
            let config = Config::new(repo).validate()?;
            print_json(&config).await?;
        }
        None => {
            print_json(&Capabilities::current()).await?;
        }
    }

    Ok(())
}

async fn serve(repo: PathBuf) -> Result<()> {
    let stdin = io::stdin();
    let mut lines = BufReader::new(stdin).lines();
    let mut stdout = io::stdout();
    let mut state = ProcessState::new(repo);
    let mut watcher: Option<RepositoryWatcher> = None;
    let mut pending_watcher: Option<JoinHandle<Result<RepositoryWatcher, gitseer::WatchError>>> =
        None;
    let mut polling_fallback = false;
    let mut poll_tick = tokio::time::interval(WATCHER_STARTUP_POLL_INTERVAL);
    poll_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        if let Some(active_watcher) = &mut watcher {
            tokio::select! {
                line = lines.next_line() => {
                    let Some(messages) = handle_line(&mut state, line?)? else {
                        write_server_message(&mut stdout, &goodbye_message("stdin closed")).await?;
                        break;
                    };
                    write_server_messages(&mut stdout, &messages).await?;
                }
                watch_event = active_watcher.next_event() => {
                    let Some(watch_event) = watch_event else {
                        write_server_message(&mut stdout, &goodbye_message("watcher stopped")).await?;
                        break;
                    };
                    if state.is_subscribed() {
                        let plan = active_watcher.refresh_plan_for_event(&watch_event);
                        if !plan.should_refresh() {
                            continue;
                        }
                        let plan = active_watcher.debounce_plan(plan, Duration::from_millis(50)).await;
                        if !plan.should_refresh() {
                            continue;
                        }
                        let refresh = refresh_repository_with_plan(
                            state.repo(),
                            state.latest_snapshot(),
                            &plan,
                            Default::default(),
                        )?;
                        let snapshot = refresh.snapshot;
                        for message in snapshot_update_messages(&mut state, snapshot) {
                            write_server_message(&mut stdout, &message).await?;
                        }
                    }
                }
            }
        } else if let Some(watcher_task) = &mut pending_watcher {
            tokio::select! {
                line = lines.next_line() => {
                    let Some(messages) = handle_line(&mut state, line?)? else {
                        write_server_message(&mut stdout, &goodbye_message("stdin closed")).await?;
                        break;
                    };
                    write_server_messages(&mut stdout, &messages).await?;
                }
                result = watcher_task => {
                    pending_watcher = None;
                    match result? {
                        Ok(started_watcher) => {
                            watcher = Some(started_watcher);
                            refresh_subscribed(&mut stdout, &mut state).await?;
                        }
                        Err(err) => {
                            eprintln!("gitseer watcher unavailable; falling back to polling: {err}");
                            polling_fallback = true;
                            refresh_subscribed(&mut stdout, &mut state).await?;
                        }
                    }
                }
                _ = poll_tick.tick() => {
                    refresh_subscribed(&mut stdout, &mut state).await?;
                }
            }
        } else if polling_fallback {
            tokio::select! {
                line = lines.next_line() => {
                    let Some(messages) = handle_line(&mut state, line?)? else {
                        write_server_message(&mut stdout, &goodbye_message("stdin closed")).await?;
                        break;
                    };
                    write_server_messages(&mut stdout, &messages).await?;
                    polling_fallback = state.is_subscribed();
                    start_watcher_if_needed(&mut pending_watcher, &state);
                }
                _ = poll_tick.tick() => {
                    refresh_subscribed(&mut stdout, &mut state).await?;
                }
            }
        } else {
            let line = lines.next_line().await?;
            let Some(messages) = handle_line(&mut state, line)? else {
                write_server_message(&mut stdout, &goodbye_message("stdin closed")).await?;
                break;
            };
            write_server_messages(&mut stdout, &messages).await?;
            start_watcher_if_needed(&mut pending_watcher, &state);
        }
    }

    Ok(())
}

fn start_watcher_if_needed(
    pending_watcher: &mut Option<JoinHandle<Result<RepositoryWatcher, gitseer::WatchError>>>,
    state: &ProcessState,
) {
    if !state.is_subscribed() || pending_watcher.is_some() {
        return;
    }

    let repo = state.repo().to_path_buf();
    *pending_watcher = Some(tokio::task::spawn_blocking(move || {
        RepositoryWatcher::new(repo)
    }));
}

async fn refresh_subscribed<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    state: &mut ProcessState,
) -> Result<()> {
    if !state.is_subscribed() {
        return Ok(());
    }

    let refresh = refresh_repository_with_plan(
        state.repo(),
        state.latest_snapshot(),
        &RefreshPlan::Full,
        Default::default(),
    )?;
    let snapshot = refresh.snapshot;
    for message in snapshot_update_messages(state, snapshot) {
        write_server_message(writer, &message).await?;
    }

    Ok(())
}

fn handle_line(
    state: &mut ProcessState,
    line: Option<String>,
) -> Result<Option<Vec<ServerMessage>>> {
    let Some(line) = line else {
        return Ok(None);
    };

    Ok(Some(handle_request(state, &line)))
}

async fn write_server_messages<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    messages: &[ServerMessage],
) -> Result<()> {
    for message in messages {
        write_server_message(writer, message).await?;
    }

    Ok(())
}

async fn write_server_message<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    message: &ServerMessage,
) -> Result<()> {
    let mut bytes = serde_json::to_vec(message)?;
    bytes.push(b'\n');
    writer.write_all(&bytes).await?;
    writer.flush().await?;
    Ok(())
}

async fn print_json<T: serde::Serialize>(value: &T) -> Result<()> {
    let mut stdout = io::stdout();
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    stdout.write_all(&bytes).await?;
    stdout.flush().await?;
    Ok(())
}
