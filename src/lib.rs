use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub mod protocol;
pub mod repository;
pub mod watch;

pub use protocol::{
    DeltaNotificationParams, ErrorObject, GoodbyeParams, JsonRpcNotification, JsonRpcRequest,
    JsonRpcResponse, ProcessState, ServerMessage, SnapshotNotificationParams, goodbye_message,
    handle_request, snapshot_update_messages,
};
pub use repository::{
    BisectState, BranchKind, BranchSummary, CommitSummary, ConflictSide, ConflictSummary,
    GitObjectKind, HeadKind, HeadState, OperationHead, OperationHeadRole, OperationKind,
    OperationState, PathDelta, PathEntry, PathEntryStatus, PathSetDelta, PathState, RefreshDomain,
    RefreshPlan, RemoteSummary, RepositoryIdentity, RepositorySnapshot, SnapshotDelta,
    SnapshotError, SnapshotOptions, SnapshotPatch, SnapshotRefresh, StashSummary, SubmoduleState,
    SubmoduleSummary, TagKind, TagSummary, UpstreamState, WorktreeSummary,
    refresh_repository_with_plan, snapshot_delta, snapshot_repository,
    snapshot_repository_with_options,
};
pub use watch::{
    RepositoryWatcher, WatchError, should_refresh_for_event, watch_roots_for_snapshot,
};

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    pub repo: PathBuf,
}

impl Config {
    pub fn new(repo: PathBuf) -> Self {
        Self { repo }
    }

    pub fn validate(self) -> Result<Self, SnapshotError> {
        snapshot_repository(&self.repo)?;
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub name: String,
    pub version: String,
    pub protocol: ProtocolCapabilities,
    pub repository: RepositoryCapabilities,
}

impl Capabilities {
    pub fn current() -> Self {
        Self {
            name: env!("CARGO_PKG_NAME").to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocol: ProtocolCapabilities::current(),
            repository: RepositoryCapabilities::current(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolCapabilities {
    pub jsonrpc: String,
    pub version: u32,
    pub transport: String,
    pub methods: Vec<String>,
    pub notifications: Vec<String>,
    pub non_goals: Vec<String>,
}

impl ProtocolCapabilities {
    pub fn current() -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            version: PROTOCOL_VERSION,
            transport: "stdio".to_string(),
            methods: vec![
                "initialize".to_string(),
                "gitseer/getSnapshot".to_string(),
                "gitseer/refresh".to_string(),
                "gitseer/subscribe".to_string(),
                "gitseer/unsubscribe".to_string(),
            ],
            notifications: vec![
                "gitseer/snapshot".to_string(),
                "gitseer/delta".to_string(),
                "gitseer/goodbye".to_string(),
            ],
            non_goals: vec![
                "multi-repository service mode".to_string(),
                "OpenRepository/CloseRepository protocol".to_string(),
                "git mutation API".to_string(),
            ],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryCapabilities {
    pub single_repository_process: bool,
    pub libgit2_backed: bool,
    pub filesystem_watch: bool,
}

impl RepositoryCapabilities {
    pub fn current() -> Self {
        Self {
            single_repository_process: true,
            libgit2_backed: true,
            filesystem_watch: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_document_single_repo_json_rpc_boundary() {
        let capabilities = Capabilities::current();

        assert_eq!(capabilities.protocol.jsonrpc, "2.0");
        assert_eq!(capabilities.protocol.transport, "stdio");
        assert!(capabilities.repository.single_repository_process);
        assert!(
            capabilities
                .protocol
                .non_goals
                .contains(&"multi-repository service mode".to_string())
        );
        assert!(
            !capabilities
                .protocol
                .methods
                .contains(&"OpenRepository".to_string())
        );
        assert!(
            !capabilities
                .protocol
                .methods
                .contains(&"CloseRepository".to_string())
        );
    }

    #[test]
    fn config_records_startup_repo_path() {
        let config = Config::new(PathBuf::from("/tmp/repo"));

        assert_eq!(config.repo, PathBuf::from("/tmp/repo"));
    }

    #[test]
    fn config_validation_rejects_non_repository_paths() {
        let temp = tempfile::TempDir::new().unwrap();
        let error = Config::new(temp.path().to_path_buf())
            .validate()
            .unwrap_err();

        assert!(matches!(error, SnapshotError::NotRepository { .. }));
    }
}
