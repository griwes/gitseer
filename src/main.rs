use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use clap::{Parser, Subcommand};
use gitseer::{
    Capabilities, Config, ProcessState, RepositoryWatcher, ServerMessage, goodbye_message,
    handle_request, refresh_repository_with_plan, snapshot_repository, snapshot_update_messages,
};
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};

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

    loop {
        if let Some(active_watcher) = &mut watcher {
            tokio::select! {
                line = lines.next_line() => {
                    if !handle_line(&mut stdout, &mut state, line?).await? {
                        break;
                    }
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
        } else {
            let line = lines.next_line().await?;
            if !handle_line(&mut stdout, &mut state, line).await? {
                break;
            }

            if state.is_subscribed() {
                watcher = Some(RepositoryWatcher::new(state.repo())?);
            }
        }
    }

    Ok(())
}

async fn handle_line<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    state: &mut ProcessState,
    line: Option<String>,
) -> Result<bool> {
    let Some(line) = line else {
        write_server_message(writer, &goodbye_message("stdin closed")).await?;
        return Ok(false);
    };
    for message in handle_request(state, &line) {
        write_server_message(writer, &message).await?;
    }
    Ok(true)
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
