use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    Capabilities, RepositorySnapshot, SnapshotDelta, SnapshotOptions, SnapshotPatch,
    snapshot_delta, snapshot_repository, snapshot_repository_with_options,
};

const JSONRPC_VERSION: &str = "2.0";
const PARSE_ERROR: i64 = -32700;
const INVALID_PARAMS: i64 = -32602;
const METHOD_NOT_FOUND: i64 = -32601;
const INTERNAL_ERROR: i64 = -32603;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
    #[serde(default)]
    pub id: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorObject>,
    pub id: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorObject {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoodbyeParams {
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotNotificationParams {
    pub version: u64,
    pub snapshot: RepositorySnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeltaNotificationParams {
    pub previous_version: u64,
    pub version: u64,
    pub delta: SnapshotDelta,
    pub patch: SnapshotPatch,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotRequestParams {
    #[serde(default)]
    include_ignored: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ServerMessage {
    Response(JsonRpcResponse),
    Notification(JsonRpcNotification),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessState {
    repo: PathBuf,
    subscribed: bool,
    latest_snapshot: Option<RepositorySnapshot>,
    latest_version: u64,
}

impl ProcessState {
    pub fn new(repo: impl Into<PathBuf>) -> Self {
        Self {
            repo: repo.into(),
            subscribed: false,
            latest_snapshot: None,
            latest_version: 0,
        }
    }

    pub fn repo(&self) -> &Path {
        &self.repo
    }

    pub fn is_subscribed(&self) -> bool {
        self.subscribed
    }

    pub fn latest_snapshot(&self) -> Option<&RepositorySnapshot> {
        self.latest_snapshot.as_ref()
    }

    fn next_version(&mut self) -> u64 {
        self.latest_version += 1;
        self.latest_version
    }

    pub fn record_baseline(&mut self, snapshot: RepositorySnapshot) -> SnapshotNotificationParams {
        let version = self.next_version();
        self.latest_snapshot = Some(snapshot);
        SnapshotNotificationParams {
            version,
            snapshot: self.latest_snapshot.as_ref().unwrap().clone(),
        }
    }

    pub fn record_refresh(&mut self, snapshot: RepositorySnapshot) -> SnapshotNotificationParams {
        self.record_baseline(snapshot)
    }

    pub fn record_update(
        &mut self,
        snapshot: RepositorySnapshot,
    ) -> Option<DeltaNotificationParams> {
        let Some(previous) = self.latest_snapshot.as_ref() else {
            self.record_baseline(snapshot);
            return None;
        };

        let delta = snapshot_delta(previous, &snapshot);
        if !delta.has_changes() {
            self.latest_snapshot = Some(snapshot);
            return None;
        }

        let previous_version = self.latest_version;
        let version = self.next_version();
        let patch = SnapshotPatch::from_delta(&snapshot, &delta);
        self.latest_snapshot = Some(snapshot);
        Some(DeltaNotificationParams {
            previous_version,
            version,
            delta,
            patch,
        })
    }
}

pub fn handle_request(state: &mut ProcessState, line: &str) -> Vec<ServerMessage> {
    let request_value = match serde_json::from_str::<Value>(line) {
        Ok(value) => value,
        Err(error) => {
            return vec![ServerMessage::Response(error_response(
                Value::Null,
                PARSE_ERROR,
                "parse error",
                Some(error.to_string()),
            ))];
        }
    };
    let is_notification = !request_value
        .as_object()
        .is_some_and(|object| object.contains_key("id"));
    let request = match serde_json::from_value::<JsonRpcRequest>(request_value) {
        Ok(request) => request,
        Err(error) => {
            return vec![ServerMessage::Response(error_response(
                Value::Null,
                PARSE_ERROR,
                "parse error",
                Some(error.to_string()),
            ))];
        }
    };

    let id = request.id.clone().unwrap_or(Value::Null);
    let response = match request.method.as_str() {
        "initialize" => success_response(id, json!(Capabilities::current())),
        "gitseer/getSnapshot" => match snapshot_options(&request.params) {
            Ok(options) => match snapshot_repository_with_options(state.repo(), options) {
                Ok(snapshot) => success_response(id, json!(snapshot)),
                Err(error) => error_response(
                    id,
                    INTERNAL_ERROR,
                    "repository snapshot failed",
                    Some(error.to_string()),
                ),
            },
            Err(error) => error_response(id, error.code, error.message, error.data),
        },
        "gitseer/refresh" => match snapshot_options(&request.params) {
            Ok(options) => match snapshot_repository_with_options(state.repo(), options) {
                Ok(snapshot) => {
                    let params = state.record_refresh(snapshot);
                    success_response(id, json!(params))
                }
                Err(error) => error_response(
                    id,
                    INTERNAL_ERROR,
                    "repository snapshot failed",
                    Some(error.to_string()),
                ),
            },
            Err(error) => error_response(id, error.code, error.message, error.data),
        },
        "gitseer/subscribe" => {
            state.subscribed = true;
            let mut messages = vec![ServerMessage::Response(success_response(
                id,
                json!({ "subscribed": true }),
            ))];
            if let Ok(snapshot) = snapshot_repository(state.repo()) {
                let params = state.record_baseline(snapshot);
                messages.push(ServerMessage::Notification(JsonRpcNotification {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    method: "gitseer/snapshot".to_string(),
                    params: Some(json!(params)),
                }));
            }
            return response_messages(messages, is_notification);
        }
        "gitseer/unsubscribe" => {
            state.subscribed = false;
            success_response(id, json!({ "subscribed": false }))
        }
        "OpenRepository" | "CloseRepository" => error_response(
            id,
            METHOD_NOT_FOUND,
            "multi-repository service mode is not supported",
            None,
        ),
        _ => error_response(id, METHOD_NOT_FOUND, "method not found", None),
    };

    response_messages(vec![ServerMessage::Response(response)], is_notification)
}

fn response_messages(messages: Vec<ServerMessage>, is_notification: bool) -> Vec<ServerMessage> {
    if is_notification {
        messages
            .into_iter()
            .filter(|message| matches!(message, ServerMessage::Notification(_)))
            .collect()
    } else {
        messages
    }
}

pub fn snapshot_update_messages(
    state: &mut ProcessState,
    snapshot: RepositorySnapshot,
) -> Vec<ServerMessage> {
    if state.latest_snapshot().is_none() {
        let params = state.record_baseline(snapshot);
        return vec![ServerMessage::Notification(JsonRpcNotification {
            jsonrpc: JSONRPC_VERSION.to_string(),
            method: "gitseer/snapshot".to_string(),
            params: Some(json!(params)),
        })];
    }

    match state.record_update(snapshot) {
        Some(delta) => vec![ServerMessage::Notification(JsonRpcNotification {
            jsonrpc: JSONRPC_VERSION.to_string(),
            method: "gitseer/delta".to_string(),
            params: Some(json!(delta)),
        })],
        None => Vec::new(),
    }
}

pub fn goodbye_message(reason: impl Into<String>) -> ServerMessage {
    ServerMessage::Notification(JsonRpcNotification {
        jsonrpc: JSONRPC_VERSION.to_string(),
        method: "gitseer/goodbye".to_string(),
        params: Some(json!(GoodbyeParams {
            reason: reason.into()
        })),
    })
}

fn success_response(id: Value, result: Value) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        result: Some(result),
        error: None,
        id,
    }
}

fn error_response(
    id: Value,
    code: i64,
    message: impl Into<String>,
    data: Option<String>,
) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        result: None,
        error: Some(ErrorObject {
            code,
            message: message.into(),
            data,
        }),
        id,
    }
}

fn snapshot_options(params: &Option<Value>) -> Result<SnapshotOptions, ErrorObject> {
    let Some(params) = params else {
        return Ok(SnapshotOptions::default());
    };
    if params.is_null() {
        return Ok(SnapshotOptions::default());
    }
    let params =
        serde_json::from_value::<SnapshotRequestParams>(params.clone()).map_err(|error| {
            ErrorObject {
                code: INVALID_PARAMS,
                message: "invalid snapshot params".to_string(),
                data: Some(error.to_string()),
            }
        })?;
    Ok(SnapshotOptions {
        include_ignored: params.include_ignored,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use notify::Event;
    use notify::event::EventKind;
    use serde_json::json;
    use tempfile::TempDir;

    use crate::watch::refresh_plan_for_event;
    use crate::{HeadKind, RefreshDomain, RefreshPlan, refresh_repository_with_plan};

    use super::*;

    #[test]
    fn initialize_returns_capabilities() {
        let repo = TestRepo::new();
        let mut state = ProcessState::new(repo.path());

        let messages = handle_request(
            &mut state,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#,
        );

        let response = only_response(messages);
        assert!(response.error.is_none());
        assert_eq!(response.result.unwrap()["protocol"]["jsonrpc"], "2.0");
    }

    #[test]
    fn get_snapshot_reads_process_repository() {
        let repo = TestRepo::new();
        repo.write("README.md", "hello\n");
        repo.git(["add", "README.md"]);
        repo.git(["commit", "-m", "initial"]);
        let mut state = ProcessState::new(repo.path());

        let messages = handle_request(
            &mut state,
            r#"{"jsonrpc":"2.0","id":"snapshot","method":"gitseer/getSnapshot"}"#,
        );

        let response = only_response(messages);
        assert!(response.error.is_none());
        assert_eq!(response.id, json!("snapshot"));
        assert_eq!(response.result.unwrap()["head"]["kind"], "attached");
    }

    #[test]
    fn refresh_records_versioned_resync_snapshot() {
        let repo = TestRepo::new();
        repo.write("README.md", "hello\n");
        repo.git(["add", "README.md"]);
        repo.git(["commit", "-m", "initial"]);
        let mut state = ProcessState::new(repo.path());

        let messages = handle_request(
            &mut state,
            r#"{"jsonrpc":"2.0","id":"refresh","method":"gitseer/refresh"}"#,
        );

        let response = only_response(messages);
        let result = response.result.unwrap();
        assert_eq!(result["version"], json!(1));
        assert_eq!(result["snapshot"]["head"]["kind"], json!("attached"));
    }

    #[test]
    fn get_snapshot_does_not_change_delta_version_baseline() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        let mut state = ProcessState::new(repo.path());
        handle_request(
            &mut state,
            r#"{"jsonrpc":"2.0","id":"subscribe","method":"gitseer/subscribe"}"#,
        );

        let messages = handle_request(
            &mut state,
            r#"{"jsonrpc":"2.0","id":"snapshot","method":"gitseer/getSnapshot"}"#,
        );
        assert!(only_response(messages).error.is_none());

        repo.write("tracked.txt", "changed\n");
        let changed = snapshot_repository(repo.path()).unwrap();
        let messages = snapshot_update_messages(&mut state, changed);

        match &messages[0] {
            ServerMessage::Notification(notification) => {
                assert_eq!(
                    notification.params.as_ref().unwrap()["previousVersion"],
                    json!(1)
                );
                assert_eq!(notification.params.as_ref().unwrap()["version"], json!(2));
            }
            ServerMessage::Response(_) => panic!("expected delta notification"),
        }
    }

    #[test]
    fn get_snapshot_accepts_include_ignored_param() {
        let repo = TestRepo::new();
        repo.write(".gitignore", "ignored.txt\n");
        repo.git(["add", ".gitignore"]);
        repo.git(["commit", "-m", "ignore rules"]);
        repo.write("ignored.txt", "ignored\n");
        let mut state = ProcessState::new(repo.path());

        let messages = handle_request(
            &mut state,
            r#"{"jsonrpc":"2.0","id":"snapshot","method":"gitseer/getSnapshot","params":{"includeIgnored":true}}"#,
        );

        let response = only_response(messages);
        assert!(response.error.is_none());
        assert_eq!(
            response.result.unwrap()["paths"]["ignored"],
            json!(["ignored.txt"])
        );
    }

    #[test]
    fn get_snapshot_rejects_invalid_params() {
        let repo = TestRepo::new();
        let mut state = ProcessState::new(repo.path());

        let messages = handle_request(
            &mut state,
            r#"{"jsonrpc":"2.0","id":"snapshot","method":"gitseer/getSnapshot","params":{"includeIgnored":"yes"}}"#,
        );

        let response = only_response(messages);
        let error = response.error.unwrap();
        assert_eq!(error.code, INVALID_PARAMS);
        assert_eq!(error.message, "invalid snapshot params");
    }

    #[test]
    fn get_snapshot_accepts_null_params_as_default_options() {
        let repo = TestRepo::new();
        let mut state = ProcessState::new(repo.path());

        let messages = handle_request(
            &mut state,
            r#"{"jsonrpc":"2.0","id":"snapshot","method":"gitseer/getSnapshot","params":null}"#,
        );

        let response = only_response(messages);
        assert!(response.error.is_none());
    }

    #[test]
    fn idless_requests_are_handled_as_notifications_without_responses() {
        let repo = TestRepo::new();
        repo.write("README.md", "hello\n");
        repo.git(["add", "README.md"]);
        repo.git(["commit", "-m", "initial"]);
        let mut state = ProcessState::new(repo.path());

        let messages = handle_request(
            &mut state,
            r#"{"jsonrpc":"2.0","method":"gitseer/getSnapshot"}"#,
        );

        assert!(messages.is_empty());
    }

    #[test]
    fn explicit_null_id_still_receives_a_response() {
        let repo = TestRepo::new();
        repo.write("README.md", "hello\n");
        repo.git(["add", "README.md"]);
        repo.git(["commit", "-m", "initial"]);
        let mut state = ProcessState::new(repo.path());

        let messages = handle_request(
            &mut state,
            r#"{"jsonrpc":"2.0","id":null,"method":"gitseer/getSnapshot"}"#,
        );

        let response = only_response(messages);
        assert_eq!(response.id, Value::Null);
        assert!(response.error.is_none());
    }

    #[test]
    fn idless_subscribe_preserves_server_notifications_only() {
        let repo = TestRepo::new();
        repo.write("README.md", "hello\n");
        repo.git(["add", "README.md"]);
        repo.git(["commit", "-m", "initial"]);
        let mut state = ProcessState::new(repo.path());

        let messages = handle_request(
            &mut state,
            r#"{"jsonrpc":"2.0","method":"gitseer/subscribe"}"#,
        );

        assert!(state.is_subscribed());
        assert_eq!(messages.len(), 1);
        match &messages[0] {
            ServerMessage::Notification(notification) => {
                assert_eq!(notification.method, "gitseer/snapshot");
            }
            ServerMessage::Response(_) => panic!("expected notification only"),
        }
    }

    #[test]
    fn subscribe_toggles_state_and_emits_snapshot_notification() {
        let repo = TestRepo::new();
        repo.write("README.md", "hello\n");
        repo.git(["add", "README.md"]);
        repo.git(["commit", "-m", "initial"]);
        let mut state = ProcessState::new(repo.path());

        let messages = handle_request(
            &mut state,
            r#"{"jsonrpc":"2.0","id":2,"method":"gitseer/subscribe"}"#,
        );

        assert!(state.is_subscribed());
        assert_eq!(messages.len(), 2);
        assert!(matches!(messages[0], ServerMessage::Response(_)));
        match &messages[1] {
            ServerMessage::Notification(notification) => {
                assert_eq!(notification.method, "gitseer/snapshot");
                assert_eq!(notification.params.as_ref().unwrap()["version"], json!(1));
                assert_eq!(
                    notification.params.as_ref().unwrap()["snapshot"]["head"]["kind"],
                    json!("attached")
                );
            }
            ServerMessage::Response(_) => panic!("expected snapshot notification"),
        }
    }

    #[test]
    fn snapshot_update_emits_delta_after_baseline_snapshot() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = snapshot_repository(repo.path()).unwrap();
        state.record_baseline(baseline);

        repo.write("tracked.txt", "changed\n");
        let changed = snapshot_repository(repo.path()).unwrap();
        let messages = snapshot_update_messages(&mut state, changed);

        assert_eq!(messages.len(), 1);
        match &messages[0] {
            ServerMessage::Notification(notification) => {
                assert_eq!(notification.method, "gitseer/delta");
                assert_eq!(
                    notification.params.as_ref().unwrap()["previousVersion"],
                    json!(1)
                );
                assert_eq!(notification.params.as_ref().unwrap()["version"], json!(2));
                assert_eq!(
                    notification.params.as_ref().unwrap()["delta"]["paths"]["unstaged"]["added"],
                    json!(["tracked.txt"])
                );
                assert_eq!(
                    notification.params.as_ref().unwrap()["patch"]["paths"]["unstaged"],
                    json!(["tracked.txt"])
                );
            }
            ServerMessage::Response(_) => panic!("expected delta notification"),
        }
    }

    #[test]
    fn snapshot_update_without_baseline_emits_seed_snapshot() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        let mut state = ProcessState::new(repo.path());

        let snapshot = snapshot_repository(repo.path()).unwrap();
        let messages = snapshot_update_messages(&mut state, snapshot);

        assert_eq!(messages.len(), 1);
        match &messages[0] {
            ServerMessage::Notification(notification) => {
                assert_eq!(notification.method, "gitseer/snapshot");
                assert_eq!(notification.params.as_ref().unwrap()["version"], json!(1));
            }
            ServerMessage::Response(_) => panic!("expected snapshot notification"),
        }
    }

    #[test]
    fn snapshot_update_includes_identity_patch_when_identity_changes() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = snapshot_repository(repo.path()).unwrap();
        state.record_baseline(baseline.clone());
        let mut changed = baseline;
        changed.identity.namespace = Some("test-namespace".to_string());

        let messages = snapshot_update_messages(&mut state, changed);

        assert_eq!(messages.len(), 1);
        match &messages[0] {
            ServerMessage::Notification(notification) => {
                assert_eq!(notification.method, "gitseer/delta");
                assert_eq!(
                    notification.params.as_ref().unwrap()["delta"]["identityChanged"],
                    json!(true)
                );
                assert_eq!(
                    notification.params.as_ref().unwrap()["patch"]["identity"]["namespace"],
                    json!("test-namespace")
                );
            }
            ServerMessage::Response(_) => panic!("expected delta notification"),
        }
    }

    #[test]
    fn snapshot_update_omits_empty_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = snapshot_repository(repo.path()).unwrap();
        state.record_baseline(baseline);

        let unchanged = snapshot_repository(repo.path()).unwrap();
        let messages = snapshot_update_messages(&mut state, unchanged);

        assert!(messages.is_empty());
    }

    #[test]
    fn request_snapshots_do_not_affect_subscription_delta_baseline() {
        let repo = TestRepo::new();
        repo.write(".gitignore", "ignored.txt\n");
        repo.git(["add", ".gitignore"]);
        repo.git(["commit", "-m", "ignore rules"]);
        repo.write("ignored.txt", "ignored\n");
        let mut state = ProcessState::new(repo.path());

        let messages = handle_request(
            &mut state,
            r#"{"jsonrpc":"2.0","id":"subscribe","method":"gitseer/subscribe"}"#,
        );
        assert_eq!(messages.len(), 2);

        let messages = handle_request(
            &mut state,
            r#"{"jsonrpc":"2.0","id":"snapshot","method":"gitseer/getSnapshot","params":{"includeIgnored":true}}"#,
        );
        let response = only_response(messages);
        assert_eq!(
            response.result.unwrap()["paths"]["ignored"],
            json!(["ignored.txt"])
        );

        let unchanged = snapshot_repository(repo.path()).unwrap();
        let messages = snapshot_update_messages(&mut state, unchanged);

        assert!(messages.is_empty());
    }

    #[test]
    fn command_shape_edit_tracked_file_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);

        repo.write("tracked.txt", "changed\n");
        let (plan, delta) =
            update_from_watch_event(&mut state, event_for(repo.path().join("tracked.txt")));

        assert_eq!(plan, RefreshPlan::domains([RefreshDomain::Paths]));
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert_eq!(delta.delta.paths.unstaged.added, vec!["tracked.txt"]);
        assert_eq!(delta.delta.paths.unstaged.removed, Vec::<String>::new());
        assert!(
            delta
                .delta
                .paths
                .entries_changed
                .contains(&"tracked.txt".to_string())
        );
        assert_eq!(
            delta.patch.paths.as_ref().unwrap().unstaged,
            vec!["tracked.txt"]
        );

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_create_untracked_file_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);

        repo.write("new.txt", "new\n");
        let (plan, delta) =
            update_from_watch_event(&mut state, event_for(repo.path().join("new.txt")));

        assert_eq!(plan, RefreshPlan::domains([RefreshDomain::Paths]));
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert_eq!(delta.delta.paths.untracked.added, vec!["new.txt"]);
        assert_eq!(delta.delta.paths.untracked.removed, Vec::<String>::new());
        assert_eq!(
            delta.patch.paths.as_ref().unwrap().untracked,
            vec!["new.txt"]
        );

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_delete_tracked_file_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);

        fs::remove_file(repo.path().join("tracked.txt")).unwrap();
        let (plan, delta) =
            update_from_watch_event(&mut state, event_for(repo.path().join("tracked.txt")));

        assert_eq!(plan, RefreshPlan::domains([RefreshDomain::Paths]));
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert_eq!(delta.delta.paths.unstaged.added, vec!["tracked.txt"]);
        assert_eq!(delta.delta.paths.unstaged.removed, Vec::<String>::new());
        assert!(
            delta
                .delta
                .paths
                .entries_changed
                .contains(&"tracked.txt".to_string())
        );
        assert_eq!(
            delta.patch.paths.as_ref().unwrap().unstaged,
            vec!["tracked.txt"]
        );

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_add_path_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.write("tracked.txt", "changed\n");
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let index_path = baseline.snapshot.identity.git_dir.join("index");

        repo.git(["add", "tracked.txt"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(index_path));

        assert_eq!(plan, RefreshPlan::domains([RefreshDomain::Paths]));
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert_eq!(delta.delta.paths.staged.added, vec!["tracked.txt"]);
        assert_eq!(delta.delta.paths.unstaged.removed, vec!["tracked.txt"]);
        assert!(
            delta
                .delta
                .paths
                .entries_changed
                .contains(&"tracked.txt".to_string())
        );
        let paths = delta.patch.paths.as_ref().unwrap();
        assert_eq!(paths.staged, vec!["tracked.txt"]);
        assert!(paths.unstaged.is_empty());

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_add_all_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.write("tracked.txt", "changed\n");
        repo.write("new.txt", "new\n");
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let index_path = baseline.snapshot.identity.git_dir.join("index");

        repo.git(["add", "-A"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(index_path));

        assert_eq!(plan, RefreshPlan::domains([RefreshDomain::Paths]));
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert_eq!(
            delta.delta.paths.staged.added,
            vec!["new.txt", "tracked.txt"]
        );
        assert_eq!(delta.delta.paths.unstaged.removed, vec!["tracked.txt"]);
        assert_eq!(delta.delta.paths.untracked.removed, vec!["new.txt"]);
        let paths = delta.patch.paths.as_ref().unwrap();
        assert_eq!(paths.staged, vec!["new.txt", "tracked.txt"]);
        assert!(paths.unstaged.is_empty());
        assert!(paths.untracked.is_empty());

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_restore_path_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.write("tracked.txt", "changed\n");
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);

        repo.git(["restore", "tracked.txt"]);
        let (plan, delta) =
            update_from_watch_event(&mut state, event_for(repo.path().join("tracked.txt")));

        assert_eq!(plan, RefreshPlan::domains([RefreshDomain::Paths]));
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert_eq!(delta.delta.paths.unstaged.removed, vec!["tracked.txt"]);
        assert!(
            delta
                .delta
                .paths
                .entries_changed
                .contains(&"tracked.txt".to_string())
        );
        let paths = delta.patch.paths.as_ref().unwrap();
        assert!(paths.staged.is_empty());
        assert!(paths.unstaged.is_empty());
        assert!(paths.untracked.is_empty());

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_restore_staged_path_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.write("tracked.txt", "changed\n");
        repo.git(["add", "tracked.txt"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let index_path = baseline.snapshot.identity.git_dir.join("index");

        repo.git(["restore", "--staged", "tracked.txt"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(index_path));

        assert_eq!(plan, RefreshPlan::domains([RefreshDomain::Paths]));
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert_eq!(delta.delta.paths.staged.removed, vec!["tracked.txt"]);
        assert_eq!(delta.delta.paths.unstaged.added, vec!["tracked.txt"]);
        assert!(
            delta
                .delta
                .paths
                .entries_changed
                .contains(&"tracked.txt".to_string())
        );
        let paths = delta.patch.paths.as_ref().unwrap();
        assert!(paths.staged.is_empty());
        assert_eq!(paths.unstaged, vec!["tracked.txt"]);

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_rm_path_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let index_path = baseline.snapshot.identity.git_dir.join("index");

        repo.git(["rm", "tracked.txt"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(index_path));

        assert_eq!(plan, RefreshPlan::domains([RefreshDomain::Paths]));
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert_eq!(delta.delta.paths.staged.added, vec!["tracked.txt"]);
        assert!(
            delta
                .delta
                .paths
                .entries_changed
                .contains(&"tracked.txt".to_string())
        );
        let paths = delta.patch.paths.as_ref().unwrap();
        assert_eq!(paths.staged, vec!["tracked.txt"]);
        assert!(paths.unstaged.is_empty());
        assert!(paths.untracked.is_empty());

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_mv_path_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("old.txt", "base\n");
        repo.git(["add", "old.txt"]);
        repo.git(["commit", "-m", "initial"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let index_path = baseline.snapshot.identity.git_dir.join("index");

        repo.git(["mv", "old.txt", "new.txt"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(index_path));

        assert_eq!(plan, RefreshPlan::domains([RefreshDomain::Paths]));
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert_eq!(delta.delta.paths.staged.added, vec!["old.txt"]);
        assert!(
            delta
                .delta
                .paths
                .entries_changed
                .contains(&"old.txt".to_string())
        );
        let paths = delta.patch.paths.as_ref().unwrap();
        assert_eq!(paths.staged, vec!["old.txt"]);
        assert!(paths.unstaged.is_empty());
        assert!(paths.untracked.is_empty());
        let entry = paths
            .entries
            .iter()
            .find(|entry| entry.path == "old.txt")
            .unwrap();
        assert_eq!(entry.staged_new_path.as_deref(), Some("new.txt"));

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_clean_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.write("untracked.txt", "untracked\n");
        repo.write("scratch/nested.txt", "nested\n");
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);

        repo.git(["clean", "-fd"]);
        let (plan, delta) =
            update_from_watch_event(&mut state, event_for(repo.path().join("untracked.txt")));

        assert_eq!(plan, RefreshPlan::domains([RefreshDomain::Paths]));
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert_eq!(
            delta.delta.paths.untracked.removed,
            vec!["scratch/nested.txt", "untracked.txt"]
        );
        let paths = delta.patch.paths.as_ref().unwrap();
        assert!(paths.staged.is_empty());
        assert!(paths.unstaged.is_empty());
        assert!(paths.untracked.is_empty());

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_checkout_path_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.write("tracked.txt", "changed\n");
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);

        repo.git(["checkout", "--", "tracked.txt"]);
        let (plan, delta) =
            update_from_watch_event(&mut state, event_for(repo.path().join("tracked.txt")));

        assert_eq!(plan, RefreshPlan::domains([RefreshDomain::Paths]));
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert_eq!(delta.delta.paths.unstaged.removed, vec!["tracked.txt"]);
        assert!(
            delta
                .delta
                .paths
                .entries_changed
                .contains(&"tracked.txt".to_string())
        );
        let paths = delta.patch.paths.as_ref().unwrap();
        assert!(paths.staged.is_empty());
        assert!(paths.unstaged.is_empty());
        assert!(paths.untracked.is_empty());

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_update_index_assume_unchanged_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let index_path = baseline.snapshot.identity.git_dir.join("index");

        repo.git(["update-index", "--assume-unchanged", "tracked.txt"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(index_path));

        assert_eq!(plan, RefreshPlan::domains([RefreshDomain::Paths]));
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert_eq!(delta.delta.paths.entries_changed, vec!["tracked.txt"]);
        let paths = delta.patch.paths.as_ref().unwrap();
        let entry = paths
            .entries
            .iter()
            .find(|entry| entry.path == "tracked.txt")
            .unwrap();
        assert!(entry.status.assume_unchanged);
        assert!(!entry.status.skip_worktree);

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_update_index_no_assume_unchanged_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git(["update-index", "--assume-unchanged", "tracked.txt"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let index_path = baseline.snapshot.identity.git_dir.join("index");

        repo.git(["update-index", "--no-assume-unchanged", "tracked.txt"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(index_path));

        assert_eq!(plan, RefreshPlan::domains([RefreshDomain::Paths]));
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert_eq!(delta.delta.paths.entries_changed, vec!["tracked.txt"]);
        let paths = delta.patch.paths.as_ref().unwrap();
        assert!(paths.entries.is_empty());

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_update_index_skip_worktree_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let index_path = baseline.snapshot.identity.git_dir.join("index");

        repo.git(["update-index", "--skip-worktree", "tracked.txt"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(index_path));

        assert_eq!(plan, RefreshPlan::domains([RefreshDomain::Paths]));
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert_eq!(delta.delta.paths.entries_changed, vec!["tracked.txt"]);
        let paths = delta.patch.paths.as_ref().unwrap();
        let entry = paths
            .entries
            .iter()
            .find(|entry| entry.path == "tracked.txt")
            .unwrap();
        assert!(!entry.status.assume_unchanged);
        assert!(entry.status.skip_worktree);

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_update_index_no_skip_worktree_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git(["update-index", "--skip-worktree", "tracked.txt"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let index_path = baseline.snapshot.identity.git_dir.join("index");

        repo.git(["update-index", "--no-skip-worktree", "tracked.txt"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(index_path));

        assert_eq!(plan, RefreshPlan::domains([RefreshDomain::Paths]));
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert_eq!(delta.delta.paths.entries_changed, vec!["tracked.txt"]);
        let paths = delta.patch.paths.as_ref().unwrap();
        assert!(paths.entries.is_empty());

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_add_root_gitignore_pattern_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.write("build/output.log", "artifact\n");
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);

        repo.write(".gitignore", "build/\n");
        let (plan, delta) =
            update_from_watch_event(&mut state, event_for(repo.path().join(".gitignore")));

        assert_eq!(plan, RefreshPlan::domains([RefreshDomain::Paths]));
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert_eq!(delta.delta.paths.untracked.added, vec![".gitignore"]);
        assert_eq!(
            delta.delta.paths.untracked.removed,
            vec!["build/output.log"]
        );
        let paths = delta.patch.paths.as_ref().unwrap();
        assert_eq!(paths.untracked, vec![".gitignore"]);

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_modify_root_gitignore_pattern_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.write(".gitignore", "build/\n");
        repo.git(["add", "tracked.txt", ".gitignore"]);
        repo.git(["commit", "-m", "initial"]);
        repo.write("build/output.log", "build\n");
        repo.write("cache/output.log", "cache\n");
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);

        repo.write(".gitignore", "cache/\n");
        let (plan, delta) =
            update_from_watch_event(&mut state, event_for(repo.path().join(".gitignore")));

        assert_eq!(plan, RefreshPlan::domains([RefreshDomain::Paths]));
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert_eq!(delta.delta.paths.unstaged.added, vec![".gitignore"]);
        assert_eq!(delta.delta.paths.untracked.added, vec!["build/output.log"]);
        assert_eq!(
            delta.delta.paths.untracked.removed,
            vec!["cache/output.log"]
        );
        let paths = delta.patch.paths.as_ref().unwrap();
        assert_eq!(paths.unstaged, vec![".gitignore"]);
        assert_eq!(paths.untracked, vec!["build/output.log"]);

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_remove_root_gitignore_pattern_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.write(".gitignore", "build/\n");
        repo.git(["add", "tracked.txt", ".gitignore"]);
        repo.git(["commit", "-m", "initial"]);
        repo.write("build/output.log", "build\n");
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);

        fs::remove_file(repo.path().join(".gitignore")).unwrap();
        let (plan, delta) =
            update_from_watch_event(&mut state, event_for(repo.path().join(".gitignore")));

        assert_eq!(plan, RefreshPlan::domains([RefreshDomain::Paths]));
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert_eq!(delta.delta.paths.unstaged.added, vec![".gitignore"]);
        assert_eq!(delta.delta.paths.untracked.added, vec!["build/output.log"]);
        let paths = delta.patch.paths.as_ref().unwrap();
        assert_eq!(paths.unstaged, vec![".gitignore"]);
        assert_eq!(paths.untracked, vec!["build/output.log"]);

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_add_nested_gitignore_pattern_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.write("module/output.log", "artifact\n");
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);

        repo.write("module/.gitignore", "*.log\n");
        let (plan, delta) =
            update_from_watch_event(&mut state, event_for(repo.path().join("module/.gitignore")));

        assert_eq!(plan, RefreshPlan::domains([RefreshDomain::Paths]));
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert_eq!(delta.delta.paths.untracked.added, vec!["module/.gitignore"]);
        assert_eq!(
            delta.delta.paths.untracked.removed,
            vec!["module/output.log"]
        );
        let paths = delta.patch.paths.as_ref().unwrap();
        assert_eq!(paths.untracked, vec!["module/.gitignore"]);

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_modify_git_info_exclude_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.write("build/output.log", "artifact\n");
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let exclude_path = baseline.snapshot.identity.git_dir.join("info/exclude");

        fs::write(&exclude_path, "build/\n").unwrap();
        let (plan, delta) = update_from_watch_event(&mut state, event_for(exclude_path));

        assert_eq!(plan, RefreshPlan::domains([RefreshDomain::Paths]));
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert_eq!(
            delta.delta.paths.untracked.removed,
            vec!["build/output.log"]
        );
        let paths = delta.patch.paths.as_ref().unwrap();
        assert!(paths.untracked.is_empty());

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_modify_core_excludesfile_emits_patchable_delta() {
        let excludes_dir = TempDir::new().unwrap();
        let excludes_path = excludes_dir.path().join("global-ignore");
        fs::write(&excludes_path, "").unwrap();

        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git([
            "config",
            "core.excludesfile",
            excludes_path.to_str().unwrap(),
        ]);
        repo.write("build/output.log", "artifact\n");
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);

        fs::write(&excludes_path, "build/\n").unwrap();
        let (plan, delta) = update_from_watch_event(&mut state, event_for(excludes_path));

        assert_eq!(plan, RefreshPlan::domains([RefreshDomain::Paths]));
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert_eq!(
            delta.delta.paths.untracked.removed,
            vec!["build/output.log"]
        );
        let paths = delta.patch.paths.as_ref().unwrap();
        assert!(paths.untracked.is_empty());

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_create_ignored_build_file_emits_no_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.write(".gitignore", "build/\n");
        repo.git(["add", "tracked.txt", ".gitignore"]);
        repo.git(["commit", "-m", "initial"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);

        repo.write("build/output.log", "artifact\n");
        let plan = plan_for_watch_event(&state, event_for(repo.path().join("build/output.log")));

        assert_eq!(plan, RefreshPlan::None);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(fresh, baseline.snapshot);
    }

    #[test]
    fn command_shape_modify_ignored_build_file_emits_no_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.write(".gitignore", "build/\n");
        repo.git(["add", "tracked.txt", ".gitignore"]);
        repo.git(["commit", "-m", "initial"]);
        repo.write("build/output.log", "artifact\n");
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);

        repo.write("build/output.log", "changed\n");
        let plan = plan_for_watch_event(&state, event_for(repo.path().join("build/output.log")));

        assert_eq!(plan, RefreshPlan::None);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(fresh, baseline.snapshot);
    }

    #[test]
    fn command_shape_delete_ignored_build_file_emits_no_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.write(".gitignore", "build/\n");
        repo.git(["add", "tracked.txt", ".gitignore"]);
        repo.git(["commit", "-m", "initial"]);
        repo.write("build/output.log", "artifact\n");
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);

        fs::remove_file(repo.path().join("build/output.log")).unwrap();
        let plan = plan_for_watch_event(&state, event_for(repo.path().join("build/output.log")));

        assert_eq!(plan, RefreshPlan::None);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(fresh, baseline.snapshot);
    }

    #[test]
    fn command_shape_git_add_force_ignored_path_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.write(".gitignore", "build/\n");
        repo.git(["add", "tracked.txt", ".gitignore"]);
        repo.git(["commit", "-m", "initial"]);
        repo.write("build/output.log", "artifact\n");
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let index_path = baseline.snapshot.identity.git_dir.join("index");

        repo.git(["add", "-f", "build/output.log"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(index_path));

        assert_eq!(plan, RefreshPlan::domains([RefreshDomain::Paths]));
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert_eq!(delta.delta.paths.staged.added, vec!["build/output.log"]);
        let paths = delta.patch.paths.as_ref().unwrap();
        assert_eq!(paths.staged, vec!["build/output.log"]);
        let entry = paths
            .entries
            .iter()
            .find(|entry| entry.path == "build/output.log")
            .unwrap();
        assert!(entry.status.index_new);

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_unignore_path_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.write(".gitignore", "build/\n");
        repo.git(["add", "tracked.txt", ".gitignore"]);
        repo.git(["commit", "-m", "initial"]);
        repo.write("build/output.log", "artifact\n");
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);

        repo.write(".gitignore", "");
        let (plan, delta) =
            update_from_watch_event(&mut state, event_for(repo.path().join(".gitignore")));

        assert_eq!(plan, RefreshPlan::domains([RefreshDomain::Paths]));
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert_eq!(delta.delta.paths.unstaged.added, vec![".gitignore"]);
        assert_eq!(delta.delta.paths.untracked.added, vec!["build/output.log"]);
        let paths = delta.patch.paths.as_ref().unwrap();
        assert_eq!(paths.unstaged, vec![".gitignore"]);
        assert_eq!(paths.untracked, vec!["build/output.log"]);

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_branch_name_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let branch_ref_path = baseline.snapshot.identity.git_dir.join("refs/heads/side");

        repo.git(["branch", "side"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(branch_ref_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(!delta.delta.head_changed);
        assert!(delta.delta.branches_changed);
        assert!(
            delta
                .patch
                .branches
                .as_ref()
                .unwrap()
                .iter()
                .any(|branch| branch.name == "side")
        );

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_switch_create_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let head_path = baseline.snapshot.identity.git_dir.join("HEAD");

        repo.git(["switch", "-c", "side"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(head_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.head_changed);
        assert!(delta.delta.branches_changed);
        assert_eq!(
            delta.patch.head.as_ref().unwrap().branch.as_deref(),
            Some("side")
        );
        assert!(
            delta
                .patch
                .branches
                .as_ref()
                .unwrap()
                .iter()
                .any(|branch| branch.name == "side" && branch.is_head)
        );

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_switch_name_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git(["branch", "side"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let head_path = baseline.snapshot.identity.git_dir.join("HEAD");

        repo.git(["switch", "side"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(head_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.head_changed);
        assert!(delta.delta.branches_changed);
        assert_eq!(
            delta.patch.head.as_ref().unwrap().branch.as_deref(),
            Some("side")
        );
        assert!(
            delta
                .patch
                .branches
                .as_ref()
                .unwrap()
                .iter()
                .any(|branch| branch.name == "side" && branch.is_head)
        );

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_checkout_create_branch_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let head_path = baseline.snapshot.identity.git_dir.join("HEAD");

        repo.git(["checkout", "-b", "side"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(head_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.head_changed);
        assert!(delta.delta.branches_changed);
        assert_eq!(
            delta.patch.head.as_ref().unwrap().branch.as_deref(),
            Some("side")
        );
        assert!(
            delta
                .patch
                .branches
                .as_ref()
                .unwrap()
                .iter()
                .any(|branch| branch.name == "side" && branch.is_head)
        );

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_checkout_commit_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        let first_oid = repo.git_stdout(["rev-parse", "HEAD"]);
        repo.write("tracked.txt", "second\n");
        repo.git(["commit", "-am", "second"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let head_path = baseline.snapshot.identity.git_dir.join("HEAD");

        repo.git(["checkout", first_oid.as_str()]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(head_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.head_changed);
        assert!(delta.delta.branches_changed);
        let head = delta.patch.head.as_ref().unwrap();
        assert_eq!(head.kind, HeadKind::Detached);
        assert_eq!(head.oid.as_deref(), Some(first_oid.as_str()));

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_switch_previous_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git(["switch", "-c", "side"]);
        repo.git(["switch", "main"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let head_path = baseline.snapshot.identity.git_dir.join("HEAD");

        repo.git(["switch", "-"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(head_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.head_changed);
        assert!(delta.delta.branches_changed);
        assert_eq!(
            delta.patch.head.as_ref().unwrap().branch.as_deref(),
            Some("side")
        );
        assert!(
            delta
                .patch
                .branches
                .as_ref()
                .unwrap()
                .iter()
                .any(|branch| branch.name == "side" && branch.is_head)
        );

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_branch_rename_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git(["branch", "side"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let branch_ref_path = baseline
            .snapshot
            .identity
            .git_dir
            .join("refs/heads/renamed");

        repo.git(["branch", "-m", "side", "renamed"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(branch_ref_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(!delta.delta.head_changed);
        assert!(delta.delta.branches_changed);
        let branches = delta.patch.branches.as_ref().unwrap();
        assert!(branches.iter().any(|branch| branch.name == "renamed"));
        assert!(!branches.iter().any(|branch| branch.name == "side"));

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_branch_delete_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git(["branch", "side"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let branch_ref_path = baseline.snapshot.identity.git_dir.join("refs/heads/side");

        repo.git(["branch", "-d", "side"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(branch_ref_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(!delta.delta.head_changed);
        assert!(delta.delta.branches_changed);
        assert!(
            !delta
                .patch
                .branches
                .as_ref()
                .unwrap()
                .iter()
                .any(|branch| branch.name == "side")
        );

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_reset_soft_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        let first_oid = repo.git_stdout(["rev-parse", "HEAD"]);
        repo.write("tracked.txt", "second\n");
        repo.git(["commit", "-am", "second"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let branch_ref_path = baseline.snapshot.identity.git_dir.join("refs/heads/main");

        repo.git(["reset", "--soft", first_oid.as_str()]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(branch_ref_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.head_changed);
        assert!(delta.delta.branches_changed);
        assert_eq!(
            delta.patch.head.as_ref().unwrap().oid.as_deref(),
            Some(first_oid.as_str())
        );
        let paths = delta.patch.paths.as_ref().unwrap();
        assert_eq!(paths.staged, vec!["tracked.txt"]);
        assert!(paths.unstaged.is_empty());

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_reset_mixed_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        let first_oid = repo.git_stdout(["rev-parse", "HEAD"]);
        repo.write("tracked.txt", "second\n");
        repo.git(["commit", "-am", "second"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let branch_ref_path = baseline.snapshot.identity.git_dir.join("refs/heads/main");

        repo.git(["reset", "--mixed", first_oid.as_str()]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(branch_ref_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.head_changed);
        assert!(delta.delta.branches_changed);
        assert_eq!(
            delta.patch.head.as_ref().unwrap().oid.as_deref(),
            Some(first_oid.as_str())
        );
        let paths = delta.patch.paths.as_ref().unwrap();
        assert!(paths.staged.is_empty());
        assert_eq!(paths.unstaged, vec!["tracked.txt"]);

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_reset_hard_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        let first_oid = repo.git_stdout(["rev-parse", "HEAD"]);
        repo.write("tracked.txt", "second\n");
        repo.git(["commit", "-am", "second"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let branch_ref_path = baseline.snapshot.identity.git_dir.join("refs/heads/main");

        repo.git(["reset", "--hard", first_oid.as_str()]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(branch_ref_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.head_changed);
        assert!(delta.delta.branches_changed);
        assert!(!delta.delta.paths.has_changes());
        assert_eq!(
            delta.patch.head.as_ref().unwrap().oid.as_deref(),
            Some(first_oid.as_str())
        );
        assert!(delta.patch.paths.is_none());

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_commit_amend_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.write("tracked.txt", "amended\n");
        repo.git(["add", "tracked.txt"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let branch_ref_path = baseline.snapshot.identity.git_dir.join("refs/heads/main");
        let previous_oid = baseline.snapshot.head.oid.clone();

        repo.git(["commit", "--amend", "--no-edit"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(branch_ref_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.head_changed);
        assert!(delta.delta.branches_changed);
        assert_eq!(delta.delta.paths.staged.removed, vec!["tracked.txt"]);
        let head = delta.patch.head.as_ref().unwrap();
        assert_ne!(head.oid, previous_oid);
        let paths = delta.patch.paths.as_ref().unwrap();
        assert!(paths.staged.is_empty());
        assert!(paths.unstaged.is_empty());

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_create_local_bare_remote_repository_emits_no_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let remotes_dir = TempDir::new().unwrap();
        let remote_path = remotes_dir.path().join("origin.git");

        init_bare_repo(&remote_path);

        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(fresh, baseline.snapshot);
    }

    #[test]
    fn command_shape_git_remote_add_emits_patchable_delta() {
        let remotes_dir = TempDir::new().unwrap();
        let remote_path = remotes_dir.path().join("origin.git");
        init_bare_repo(&remote_path);

        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let config_path = baseline.snapshot.identity.git_dir.join("config");

        repo.git(["remote", "add", "origin", remote_path.to_str().unwrap()]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(config_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Remotes,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.remotes_changed);
        let remotes = delta.patch.remotes.as_ref().unwrap();
        assert_eq!(remotes.len(), 1);
        assert_eq!(remotes[0].name, "origin");
        assert_eq!(remotes[0].url.as_deref(), remote_path.to_str());

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_remote_rename_emits_patchable_delta() {
        let remotes_dir = TempDir::new().unwrap();
        let remote_path = remotes_dir.path().join("origin.git");
        init_bare_repo(&remote_path);

        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git(["remote", "add", "origin", remote_path.to_str().unwrap()]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let config_path = baseline.snapshot.identity.git_dir.join("config");

        repo.git(["remote", "rename", "origin", "upstream"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(config_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Remotes,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.remotes_changed);
        let remotes = delta.patch.remotes.as_ref().unwrap();
        assert_eq!(remotes.len(), 1);
        assert_eq!(remotes[0].name, "upstream");
        assert!(!remotes.iter().any(|remote| remote.name == "origin"));

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_remote_remove_emits_patchable_delta() {
        let remotes_dir = TempDir::new().unwrap();
        let remote_path = remotes_dir.path().join("origin.git");
        init_bare_repo(&remote_path);

        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git(["remote", "add", "origin", remote_path.to_str().unwrap()]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let config_path = baseline.snapshot.identity.git_dir.join("config");

        repo.git(["remote", "remove", "origin"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(config_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Remotes,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.remotes_changed);
        assert!(delta.patch.remotes.as_ref().unwrap().is_empty());

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_push_set_upstream_emits_patchable_delta() {
        let remotes_dir = TempDir::new().unwrap();
        let remote_path = remotes_dir.path().join("origin.git");
        init_bare_repo(&remote_path);

        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git(["remote", "add", "origin", remote_path.to_str().unwrap()]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let config_path = baseline.snapshot.identity.git_dir.join("config");

        repo.git(["push", "-u", "origin", "main"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(config_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Remotes,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.upstream_changed);
        assert!(delta.delta.branches_changed);
        let upstream = delta.patch.upstream.as_ref().unwrap().as_ref().unwrap();
        assert_eq!(upstream.name, "origin/main");
        assert_eq!(upstream.ahead, 0);
        assert_eq!(upstream.behind, 0);
        let branches = delta.patch.branches.as_ref().unwrap();
        assert!(branches.iter().any(|branch| {
            branch.name == "origin/main" && branch.kind == crate::BranchKind::Remote
        }));
        let head = branches.iter().find(|branch| branch.is_head).unwrap();
        assert_eq!(head.upstream.as_deref(), Some("origin/main"));
        assert_eq!(head.upstream_ahead, Some(0));
        assert_eq!(head.upstream_behind, Some(0));

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_branch_set_upstream_to_emits_patchable_delta() {
        let remotes_dir = TempDir::new().unwrap();
        let remote_path = remotes_dir.path().join("origin.git");
        init_bare_repo(&remote_path);

        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git(["remote", "add", "origin", remote_path.to_str().unwrap()]);
        repo.git(["push", "origin", "main"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let config_path = baseline.snapshot.identity.git_dir.join("config");
        assert!(baseline.snapshot.upstream.is_none());
        assert!(baseline.snapshot.branches.iter().any(|branch| {
            branch.name == "origin/main" && branch.kind == crate::BranchKind::Remote
        }));

        repo.git(["branch", "--set-upstream-to=origin/main", "main"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(config_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Remotes,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.upstream_changed);
        assert!(delta.delta.branches_changed);
        let upstream = delta.patch.upstream.as_ref().unwrap().as_ref().unwrap();
        assert_eq!(upstream.name, "origin/main");
        assert_eq!(upstream.ahead, 0);
        assert_eq!(upstream.behind, 0);
        let branches = delta.patch.branches.as_ref().unwrap();
        let head = branches.iter().find(|branch| branch.is_head).unwrap();
        assert_eq!(head.upstream.as_deref(), Some("origin/main"));
        assert_eq!(head.upstream_ahead, Some(0));
        assert_eq!(head.upstream_behind, Some(0));

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_branch_unset_upstream_emits_patchable_delta() {
        let remotes_dir = TempDir::new().unwrap();
        let remote_path = remotes_dir.path().join("origin.git");
        init_bare_repo(&remote_path);

        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git(["remote", "add", "origin", remote_path.to_str().unwrap()]);
        repo.git(["push", "-u", "origin", "main"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let config_path = baseline.snapshot.identity.git_dir.join("config");
        assert!(baseline.snapshot.upstream.is_some());

        repo.git(["branch", "--unset-upstream"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(config_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Remotes,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.upstream_changed);
        assert!(delta.delta.branches_changed);
        assert!(delta.patch.upstream.as_ref().unwrap().is_none());
        let branches = delta.patch.branches.as_ref().unwrap();
        let head = branches.iter().find(|branch| branch.is_head).unwrap();
        assert!(head.upstream.is_none());
        assert!(head.upstream_ahead.is_none());
        assert!(head.upstream_behind.is_none());

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_fetch_remote_emits_patchable_delta() {
        let remote = TestRepo::new();
        remote.write("remote.txt", "remote\n");
        remote.git(["add", "remote.txt"]);
        remote.git(["commit", "-m", "remote"]);

        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git(["remote", "add", "origin", remote.path().to_str().unwrap()]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let fetch_head_path = baseline.snapshot.identity.git_dir.join("FETCH_HEAD");
        assert!(
            !baseline
                .snapshot
                .branches
                .iter()
                .any(|branch| branch.name == "origin/main")
        );

        repo.git(["fetch", "origin"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(fetch_head_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Remotes
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(!delta.delta.upstream_changed);
        assert!(delta.delta.branches_changed);
        assert!(delta.delta.remotes_changed);
        let origin = delta
            .patch
            .remotes
            .as_ref()
            .unwrap()
            .iter()
            .find(|remote| remote.name == "origin")
            .unwrap();
        assert_eq!(origin.default_branch.as_deref(), Some("main"));
        let branches = delta.patch.branches.as_ref().unwrap();
        assert!(branches.iter().any(|branch| {
            branch.name == "origin/main" && branch.kind == crate::BranchKind::Remote
        }));

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_fetch_prune_remote_emits_patchable_delta() {
        let remote = TestRepo::new();
        remote.write("remote.txt", "remote\n");
        remote.git(["add", "remote.txt"]);
        remote.git(["commit", "-m", "remote"]);
        remote.git(["branch", "side"]);

        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git(["remote", "add", "origin", remote.path().to_str().unwrap()]);
        repo.git(["fetch", "origin"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let pruned_ref_path = baseline
            .snapshot
            .identity
            .git_dir
            .join("refs/remotes/origin/side");
        assert!(
            baseline
                .snapshot
                .branches
                .iter()
                .any(|branch| branch.name == "origin/side")
        );

        remote.git(["branch", "-d", "side"]);
        repo.git(["fetch", "--prune", "origin"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(pruned_ref_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Remotes
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(!delta.delta.upstream_changed);
        assert!(delta.delta.branches_changed);
        let branches = delta.patch.branches.as_ref().unwrap();
        assert!(branches.iter().any(|branch| {
            branch.name == "origin/main" && branch.kind == crate::BranchKind::Remote
        }));
        assert!(!branches.iter().any(|branch| branch.name == "origin/side"));

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_local_ahead_after_commit_emits_patchable_delta() {
        let remotes_dir = TempDir::new().unwrap();
        let remote_path = remotes_dir.path().join("origin.git");
        init_bare_repo(&remote_path);

        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git(["remote", "add", "origin", remote_path.to_str().unwrap()]);
        repo.git(["push", "-u", "origin", "main"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let branch_ref_path = baseline.snapshot.identity.git_dir.join("refs/heads/main");
        let baseline_upstream = baseline.snapshot.upstream.as_ref().unwrap();
        assert_eq!(baseline_upstream.ahead, 0);
        assert_eq!(baseline_upstream.behind, 0);

        repo.write("tracked.txt", "ahead\n");
        repo.git(["commit", "-am", "ahead"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(branch_ref_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.head_changed);
        assert!(delta.delta.upstream_changed);
        assert!(delta.delta.branches_changed);
        assert!(!delta.delta.paths.has_changes());
        let upstream = delta.patch.upstream.as_ref().unwrap().as_ref().unwrap();
        assert_eq!(upstream.name, "origin/main");
        assert_eq!(upstream.ahead, 1);
        assert_eq!(upstream.behind, 0);
        let head = delta
            .patch
            .branches
            .as_ref()
            .unwrap()
            .iter()
            .find(|branch| branch.is_head)
            .unwrap();
        assert_eq!(head.upstream_ahead, Some(1));
        assert_eq!(head.upstream_behind, Some(0));

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_local_behind_after_remote_commit_and_fetch_emits_patchable_delta() {
        let remotes_dir = TempDir::new().unwrap();
        let remote_path = remotes_dir.path().join("origin.git");
        init_bare_repo(&remote_path);

        let seed = TestRepo::new();
        seed.write("tracked.txt", "base\n");
        seed.git(["add", "tracked.txt"]);
        seed.git(["commit", "-m", "initial"]);
        seed.git(["remote", "add", "origin", remote_path.to_str().unwrap()]);
        seed.git(["push", "-u", "origin", "main"]);

        let repo = TestRepo::clone_from(&remote_path);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let remote_ref_path = baseline
            .snapshot
            .identity
            .git_dir
            .join("refs/remotes/origin/main");
        let baseline_upstream = baseline.snapshot.upstream.as_ref().unwrap();
        assert_eq!(baseline_upstream.ahead, 0);
        assert_eq!(baseline_upstream.behind, 0);

        let other = TestRepo::clone_from(&remote_path);
        other.write("tracked.txt", "remote\n");
        other.git(["commit", "-am", "remote"]);
        other.git(["push", "origin", "main"]);
        repo.git(["fetch", "origin"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(remote_ref_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Remotes
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(!delta.delta.head_changed);
        assert!(delta.delta.upstream_changed);
        assert!(delta.delta.branches_changed);
        let upstream = delta.patch.upstream.as_ref().unwrap().as_ref().unwrap();
        assert_eq!(upstream.name, "origin/main");
        assert_eq!(upstream.ahead, 0);
        assert_eq!(upstream.behind, 1);
        let head = delta
            .patch
            .branches
            .as_ref()
            .unwrap()
            .iter()
            .find(|branch| branch.is_head)
            .unwrap();
        assert_eq!(head.upstream_ahead, Some(0));
        assert_eq!(head.upstream_behind, Some(1));

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_diverged_after_local_and_remote_commits_emits_patchable_delta() {
        let remotes_dir = TempDir::new().unwrap();
        let remote_path = remotes_dir.path().join("origin.git");
        init_bare_repo(&remote_path);

        let seed = TestRepo::new();
        seed.write("tracked.txt", "base\n");
        seed.git(["add", "tracked.txt"]);
        seed.git(["commit", "-m", "initial"]);
        seed.git(["remote", "add", "origin", remote_path.to_str().unwrap()]);
        seed.git(["push", "-u", "origin", "main"]);

        let repo = TestRepo::clone_from(&remote_path);
        repo.write("local.txt", "local\n");
        repo.git(["add", "local.txt"]);
        repo.git(["commit", "-m", "local"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let remote_ref_path = baseline
            .snapshot
            .identity
            .git_dir
            .join("refs/remotes/origin/main");
        let baseline_upstream = baseline.snapshot.upstream.as_ref().unwrap();
        assert_eq!(baseline_upstream.ahead, 1);
        assert_eq!(baseline_upstream.behind, 0);

        let other = TestRepo::clone_from(&remote_path);
        other.write("remote.txt", "remote\n");
        other.git(["add", "remote.txt"]);
        other.git(["commit", "-m", "remote"]);
        other.git(["push", "origin", "main"]);
        repo.git(["fetch", "origin"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(remote_ref_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Remotes
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(!delta.delta.head_changed);
        assert!(delta.delta.upstream_changed);
        assert!(delta.delta.branches_changed);
        let upstream = delta.patch.upstream.as_ref().unwrap().as_ref().unwrap();
        assert_eq!(upstream.name, "origin/main");
        assert_eq!(upstream.ahead, 1);
        assert_eq!(upstream.behind, 1);
        let head = delta
            .patch
            .branches
            .as_ref()
            .unwrap()
            .iter()
            .find(|branch| branch.is_head)
            .unwrap();
        assert_eq!(head.upstream_ahead, Some(1));
        assert_eq!(head.upstream_behind, Some(1));

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_remote_set_url_emits_patchable_delta() {
        let remote_parent = TempDir::new().unwrap();
        let old_remote_path = remote_parent.path().join("old.git");
        let new_remote_path = remote_parent.path().join("new.git");
        init_bare_repo(&old_remote_path);
        init_bare_repo(&new_remote_path);
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git(["remote", "add", "origin", old_remote_path.to_str().unwrap()]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let config_path = baseline.snapshot.identity.git_dir.join("config");

        repo.git([
            "remote",
            "set-url",
            "origin",
            new_remote_path.to_str().unwrap(),
        ]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(config_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Remotes,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.remotes_changed);
        assert!(!delta.delta.paths.has_changes());
        let remotes = delta.patch.remotes.as_ref().unwrap();
        assert_eq!(remotes[0].name, "origin");
        assert_eq!(
            remotes[0].url.as_deref(),
            Some(new_remote_path.to_str().unwrap())
        );

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_remote_set_head_auto_emits_patchable_delta() {
        let remote_parent = TempDir::new().unwrap();
        let remote_path = remote_parent.path().join("remote.git");
        init_bare_repo(&remote_path);
        let repo = TestRepo::clone_from(&remote_path);
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git(["push", "-u", "origin", "main"]);
        repo.git(["remote", "set-head", "origin", "-d"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let remote_head_path = baseline
            .snapshot
            .identity
            .git_dir
            .join("refs/remotes/origin/HEAD");
        assert!(
            !baseline
                .snapshot
                .branches
                .iter()
                .any(|branch| branch.name == "origin/HEAD")
        );

        repo.git(["remote", "set-head", "origin", "--auto"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(remote_head_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Remotes
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.branches_changed);
        assert!(
            delta
                .patch
                .branches
                .as_ref()
                .unwrap()
                .iter()
                .any(|branch| branch.name == "origin/HEAD")
        );

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_direct_remote_config_edit_emits_patchable_delta() {
        let remote_parent = TempDir::new().unwrap();
        let old_remote_path = remote_parent.path().join("old.git");
        let new_remote_path = remote_parent.path().join("new.git");
        init_bare_repo(&old_remote_path);
        init_bare_repo(&new_remote_path);
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git(["remote", "add", "origin", old_remote_path.to_str().unwrap()]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let config_path = baseline.snapshot.identity.git_dir.join("config");

        let config = fs::read_to_string(&config_path).unwrap();
        fs::write(
            &config_path,
            config.replace(
                old_remote_path.to_str().unwrap(),
                new_remote_path.to_str().unwrap(),
            ),
        )
        .unwrap();
        let (plan, delta) = update_from_watch_event(&mut state, event_for(config_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Remotes,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.remotes_changed);
        assert!(!delta.delta.paths.has_changes());
        assert_eq!(
            delta.patch.remotes.as_ref().unwrap()[0].url.as_deref(),
            Some(new_remote_path.to_str().unwrap())
        );

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_tag_lightweight_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let tag_ref_path = baseline.snapshot.identity.git_dir.join("refs/tags/v1.0.0");

        repo.git(["tag", "v1.0.0"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(tag_ref_path));

        assert_eq!(plan, RefreshPlan::domains([RefreshDomain::Tags]));
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.tags_changed);
        let tags = delta.patch.tags.as_ref().unwrap();
        let tag = tags.iter().find(|tag| tag.name == "v1.0.0").unwrap();
        assert_eq!(tag.kind, crate::TagKind::Lightweight);

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_tag_annotated_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let tag_ref_path = baseline.snapshot.identity.git_dir.join("refs/tags/v1.0.0");

        repo.git(["tag", "-a", "v1.0.0", "-m", "release"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(tag_ref_path));

        assert_eq!(plan, RefreshPlan::domains([RefreshDomain::Tags]));
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.tags_changed);
        let tags = delta.patch.tags.as_ref().unwrap();
        let tag = tags.iter().find(|tag| tag.name == "v1.0.0").unwrap();
        assert_eq!(tag.kind, crate::TagKind::Annotated);
        assert_eq!(tag.message.as_deref(), Some("release\n"));
        assert_eq!(tag.tagger_name.as_deref(), Some("Tester"));
        assert_eq!(tag.tagger_email.as_deref(), Some("tester@example.com"));

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_tag_delete_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git(["tag", "v1.0.0"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let tag_ref_path = baseline.snapshot.identity.git_dir.join("refs/tags/v1.0.0");
        assert!(
            baseline
                .snapshot
                .tags
                .iter()
                .any(|tag| tag.name == "v1.0.0")
        );

        repo.git(["tag", "-d", "v1.0.0"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(tag_ref_path));

        assert_eq!(plan, RefreshPlan::domains([RefreshDomain::Tags]));
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.tags_changed);
        assert!(delta.patch.tags.as_ref().unwrap().is_empty());

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_pack_refs_all_emits_no_semantic_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git(["branch", "side"]);
        repo.git(["tag", "v1.0.0"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let packed_refs_path = baseline.snapshot.identity.git_dir.join("packed-refs");

        repo.git(["pack-refs", "--all"]);
        let plan = plan_for_watch_event(&state, event_for(packed_refs_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Tags
            ])
        );
        let refresh = refresh_repository_with_plan(
            state.repo(),
            Some(&baseline.snapshot),
            &plan,
            SnapshotOptions::default(),
        )
        .unwrap();
        assert_eq!(refresh.plan, plan);
        let messages = snapshot_update_messages(&mut state, refresh.snapshot);
        assert!(messages.is_empty());
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(fresh, baseline.snapshot);
    }

    #[test]
    fn command_shape_update_branch_with_packed_previous_ref_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git(["branch", "side"]);
        repo.write("tracked.txt", "second\n");
        repo.git(["commit", "-am", "second"]);
        let main_oid = repo.git_stdout(["rev-parse", "main"]);
        repo.git(["pack-refs", "--all"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let side_ref_path = baseline.snapshot.identity.git_dir.join("refs/heads/side");
        let baseline_side = baseline
            .snapshot
            .branches
            .iter()
            .find(|branch| branch.name == "side")
            .unwrap();
        assert_ne!(baseline_side.oid.as_deref(), Some(main_oid.as_str()));

        repo.git(["branch", "-f", "side", "main"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(side_ref_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(!delta.delta.head_changed);
        assert!(delta.delta.branches_changed);
        let side = delta
            .patch
            .branches
            .as_ref()
            .unwrap()
            .iter()
            .find(|branch| branch.name == "side")
            .unwrap();
        assert_eq!(side.oid.as_deref(), Some(main_oid.as_str()));

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_stash_push_message_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.write("tracked.txt", "work\n");
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let stash_ref_path = baseline.snapshot.identity.git_dir.join("refs/stash");
        assert_eq!(baseline.snapshot.paths.unstaged, vec!["tracked.txt"]);

        repo.git(["stash", "push", "-m", "save work"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(stash_ref_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths,
                RefreshDomain::Stashes
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.paths.has_changes());
        assert!(delta.delta.stashes_changed);
        assert!(delta.patch.paths.as_ref().unwrap().unstaged.is_empty());
        let stashes = delta.patch.stashes.as_ref().unwrap();
        assert_eq!(stashes.len(), 1);
        assert!(stashes[0].message.contains("save work"));

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_stash_push_include_untracked_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.write("untracked.txt", "new\n");
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let stash_ref_path = baseline.snapshot.identity.git_dir.join("refs/stash");
        assert_eq!(baseline.snapshot.paths.untracked, vec!["untracked.txt"]);

        repo.git(["stash", "push", "--include-untracked", "-m", "save all"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(stash_ref_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths,
                RefreshDomain::Stashes
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.paths.has_changes());
        assert!(delta.delta.stashes_changed);
        assert!(delta.patch.paths.as_ref().unwrap().untracked.is_empty());
        let stashes = delta.patch.stashes.as_ref().unwrap();
        assert_eq!(stashes.len(), 1);
        assert!(stashes[0].message.contains("save all"));

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_stash_pop_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.write("tracked.txt", "work\n");
        repo.git(["stash", "push", "-m", "save work"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let stash_ref_path = baseline.snapshot.identity.git_dir.join("refs/stash");
        assert_eq!(baseline.snapshot.stashes.len(), 1);
        assert!(baseline.snapshot.paths.unstaged.is_empty());

        repo.git(["stash", "pop"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(stash_ref_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths,
                RefreshDomain::Stashes
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.paths.has_changes());
        assert!(delta.delta.stashes_changed);
        assert_eq!(
            delta.patch.paths.as_ref().unwrap().unstaged,
            vec!["tracked.txt"]
        );
        assert!(delta.patch.stashes.as_ref().unwrap().is_empty());

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_stash_apply_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.write("tracked.txt", "work\n");
        repo.git(["stash", "push", "-m", "save work"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        assert_eq!(baseline.snapshot.stashes.len(), 1);
        assert!(baseline.snapshot.paths.unstaged.is_empty());

        repo.git(["stash", "apply"]);
        let (plan, delta) =
            update_from_watch_event(&mut state, event_for(repo.path().join("tracked.txt")));

        assert_eq!(plan, RefreshPlan::domains([RefreshDomain::Paths]));
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.paths.has_changes());
        assert!(!delta.delta.stashes_changed);
        assert_eq!(
            delta.patch.paths.as_ref().unwrap().unstaged,
            vec!["tracked.txt"]
        );
        assert!(delta.patch.stashes.is_none());

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_stash_drop_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.write("tracked.txt", "work\n");
        repo.git(["stash", "push", "-m", "save work"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let stash_ref_path = baseline.snapshot.identity.git_dir.join("refs/stash");
        assert_eq!(baseline.snapshot.stashes.len(), 1);
        assert!(baseline.snapshot.paths.unstaged.is_empty());

        repo.git(["stash", "drop"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(stash_ref_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths,
                RefreshDomain::Stashes
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(!delta.delta.paths.has_changes());
        assert!(delta.delta.stashes_changed);
        assert!(delta.patch.paths.is_none());
        assert!(delta.patch.stashes.as_ref().unwrap().is_empty());

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_stash_clear_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.write("tracked.txt", "first\n");
        repo.git(["stash", "push", "-m", "first"]);
        repo.write("tracked.txt", "second\n");
        repo.git(["stash", "push", "-m", "second"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let stash_ref_path = baseline.snapshot.identity.git_dir.join("refs/stash");
        assert_eq!(baseline.snapshot.stashes.len(), 2);
        assert!(baseline.snapshot.paths.unstaged.is_empty());

        repo.git(["stash", "clear"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(stash_ref_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths,
                RefreshDomain::Stashes
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(!delta.delta.paths.has_changes());
        assert!(delta.delta.stashes_changed);
        assert!(delta.patch.paths.is_none());
        assert!(delta.patch.stashes.as_ref().unwrap().is_empty());

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_stash_branch_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.write("tracked.txt", "work\n");
        repo.git(["stash", "push", "-m", "save work"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let stash_ref_path = baseline.snapshot.identity.git_dir.join("refs/stash");
        assert_eq!(baseline.snapshot.head.branch.as_deref(), Some("main"));
        assert_eq!(baseline.snapshot.stashes.len(), 1);
        assert!(baseline.snapshot.paths.unstaged.is_empty());

        repo.git(["stash", "branch", "work"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(stash_ref_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths,
                RefreshDomain::Stashes
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.head_changed);
        assert!(delta.delta.paths.has_changes());
        assert!(delta.delta.branches_changed);
        assert!(delta.delta.stashes_changed);
        assert_eq!(
            delta.patch.head.as_ref().unwrap().branch.as_deref(),
            Some("work")
        );
        let head = delta
            .patch
            .branches
            .as_ref()
            .unwrap()
            .iter()
            .find(|branch| branch.is_head)
            .unwrap();
        assert_eq!(head.name, "work");
        assert_eq!(
            delta.patch.paths.as_ref().unwrap().unstaged,
            vec!["tracked.txt"]
        );
        assert!(delta.patch.stashes.as_ref().unwrap().is_empty());

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_clean_git_merge_branch_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git(["checkout", "-b", "side"]);
        repo.write("tracked.txt", "side\n");
        repo.git(["commit", "-am", "side"]);
        let side_oid = repo.git_stdout(["rev-parse", "side"]);
        repo.git(["checkout", "main"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let main_ref_path = baseline.snapshot.identity.git_dir.join("refs/heads/main");
        assert_ne!(
            baseline.snapshot.head.oid.as_deref(),
            Some(side_oid.as_str())
        );

        repo.git(["merge", "side"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(main_ref_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.head_changed);
        assert!(delta.delta.branches_changed);
        assert!(!delta.delta.paths.has_changes());
        assert_eq!(
            delta.patch.head.as_ref().unwrap().oid.as_deref(),
            Some(side_oid.as_str())
        );
        let head = delta
            .patch
            .branches
            .as_ref()
            .unwrap()
            .iter()
            .find(|branch| branch.is_head)
            .unwrap();
        assert_eq!(head.name, "main");
        assert_eq!(head.oid.as_deref(), Some(side_oid.as_str()));

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_conflicted_git_merge_branch_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("conflict.txt", "base\n");
        repo.git(["add", "conflict.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git(["checkout", "-b", "side"]);
        repo.write("conflict.txt", "side\n");
        repo.git(["commit", "-am", "side"]);
        let side_oid = repo.git_stdout(["rev-parse", "side"]);
        repo.git(["checkout", "main"]);
        repo.write("conflict.txt", "main\n");
        repo.git(["commit", "-am", "main"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let merge_head_path = baseline.snapshot.identity.git_dir.join("MERGE_HEAD");

        repo.git_expect_failure(["merge", "side"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(merge_head_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Operation,
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(!delta.delta.head_changed);
        assert!(delta.delta.operation_changed);
        assert!(delta.delta.paths.has_changes());
        let operation = delta.patch.operation.as_ref().unwrap();
        assert_eq!(operation.kind, crate::OperationKind::Merge);
        let merge_head = operation
            .heads
            .iter()
            .find(|head| head.role == crate::OperationHeadRole::Merge)
            .unwrap();
        assert_eq!(merge_head.oid, side_oid);
        let paths = delta.patch.paths.as_ref().unwrap();
        assert_eq!(paths.conflicted, vec!["conflict.txt"]);
        assert_eq!(paths.conflicts.len(), 1);

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_merge_abort_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("conflict.txt", "base\n");
        repo.git(["add", "conflict.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git(["checkout", "-b", "side"]);
        repo.write("conflict.txt", "side\n");
        repo.git(["commit", "-am", "side"]);
        repo.git(["checkout", "main"]);
        repo.write("conflict.txt", "main\n");
        repo.git(["commit", "-am", "main"]);
        repo.git_expect_failure(["merge", "side"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let merge_head_path = baseline.snapshot.identity.git_dir.join("MERGE_HEAD");
        assert_eq!(
            baseline.snapshot.operation.kind,
            crate::OperationKind::Merge
        );
        assert_eq!(baseline.snapshot.paths.conflicted, vec!["conflict.txt"]);

        repo.git(["merge", "--abort"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(merge_head_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Operation,
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(!delta.delta.head_changed);
        assert!(delta.delta.operation_changed);
        assert!(delta.delta.paths.has_changes());
        assert_eq!(
            delta.patch.operation.as_ref().unwrap().kind,
            crate::OperationKind::Clean
        );
        let paths = delta.patch.paths.as_ref().unwrap();
        assert!(paths.conflicted.is_empty());
        assert!(paths.conflicts.is_empty());
        assert!(paths.unstaged.is_empty());

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_merge_continue_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("conflict.txt", "base\n");
        repo.git(["add", "conflict.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git(["checkout", "-b", "side"]);
        repo.write("conflict.txt", "side\n");
        repo.git(["commit", "-am", "side"]);
        repo.git(["checkout", "main"]);
        repo.write("conflict.txt", "main\n");
        repo.git(["commit", "-am", "main"]);
        let main_before_continue = repo.git_stdout(["rev-parse", "main"]);
        repo.git_expect_failure(["merge", "side"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let merge_head_path = baseline.snapshot.identity.git_dir.join("MERGE_HEAD");
        assert_eq!(
            baseline.snapshot.operation.kind,
            crate::OperationKind::Merge
        );
        assert_eq!(baseline.snapshot.paths.conflicted, vec!["conflict.txt"]);

        repo.write("conflict.txt", "resolved\n");
        repo.git(["add", "conflict.txt"]);
        repo.git(["merge", "--continue"]);
        let merge_oid = repo.git_stdout(["rev-parse", "main"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(merge_head_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Operation,
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.head_changed);
        assert!(delta.delta.operation_changed);
        assert!(delta.delta.branches_changed);
        assert!(delta.delta.paths.has_changes());
        assert_ne!(merge_oid, main_before_continue);
        assert_eq!(
            delta.patch.operation.as_ref().unwrap().kind,
            crate::OperationKind::Clean
        );
        assert_eq!(
            delta.patch.head.as_ref().unwrap().oid.as_deref(),
            Some(merge_oid.as_str())
        );
        let head = delta
            .patch
            .branches
            .as_ref()
            .unwrap()
            .iter()
            .find(|branch| branch.is_head)
            .unwrap();
        assert_eq!(head.oid.as_deref(), Some(merge_oid.as_str()));
        let paths = delta.patch.paths.as_ref().unwrap();
        assert!(paths.conflicted.is_empty());
        assert!(paths.staged.is_empty());
        assert!(paths.unstaged.is_empty());

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_clean_git_rebase_branch_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("base.txt", "base\n");
        repo.git(["add", "base.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git(["checkout", "-b", "side"]);
        repo.write("side.txt", "side\n");
        repo.git(["add", "side.txt"]);
        repo.git(["commit", "-m", "side"]);
        let side_before_rebase = repo.git_stdout(["rev-parse", "side"]);
        repo.git(["checkout", "main"]);
        repo.write("main.txt", "main\n");
        repo.git(["add", "main.txt"]);
        repo.git(["commit", "-m", "main"]);
        repo.git(["checkout", "side"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let side_ref_path = baseline.snapshot.identity.git_dir.join("refs/heads/side");
        assert_eq!(
            baseline.snapshot.head.oid.as_deref(),
            Some(side_before_rebase.as_str())
        );

        repo.git(["rebase", "main"]);
        let rebased_oid = repo.git_stdout(["rev-parse", "side"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(side_ref_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.head_changed);
        assert!(delta.delta.branches_changed);
        assert!(!delta.delta.paths.has_changes());
        assert_ne!(rebased_oid, side_before_rebase);
        assert_eq!(
            delta.patch.head.as_ref().unwrap().oid.as_deref(),
            Some(rebased_oid.as_str())
        );
        let head = delta
            .patch
            .branches
            .as_ref()
            .unwrap()
            .iter()
            .find(|branch| branch.is_head)
            .unwrap();
        assert_eq!(head.name, "side");
        assert_eq!(head.oid.as_deref(), Some(rebased_oid.as_str()));

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_conflicted_git_rebase_branch_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("conflict.txt", "base\n");
        repo.git(["add", "conflict.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git(["checkout", "-b", "side"]);
        repo.write("conflict.txt", "side\n");
        repo.git(["commit", "-am", "side"]);
        let side_oid = repo.git_stdout(["rev-parse", "side"]);
        repo.git(["checkout", "main"]);
        repo.write("conflict.txt", "main\n");
        repo.git(["commit", "-am", "main"]);
        repo.git(["checkout", "side"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let rebase_head_path = baseline.snapshot.identity.git_dir.join("REBASE_HEAD");
        assert_eq!(
            baseline.snapshot.head.oid.as_deref(),
            Some(side_oid.as_str())
        );

        repo.git_expect_failure(["rebase", "main"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(rebase_head_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Operation,
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.head_changed);
        assert!(delta.delta.operation_changed);
        assert!(delta.delta.paths.has_changes());
        let operation = delta.patch.operation.as_ref().unwrap();
        assert!(matches!(
            operation.kind,
            crate::OperationKind::Rebase
                | crate::OperationKind::RebaseInteractive
                | crate::OperationKind::RebaseMerge
        ));
        let rebase_head = operation
            .heads
            .iter()
            .find(|head| head.role == crate::OperationHeadRole::Rebase)
            .unwrap();
        assert_eq!(rebase_head.oid, side_oid);
        let paths = delta.patch.paths.as_ref().unwrap();
        assert_eq!(paths.conflicted, vec!["conflict.txt"]);
        assert_eq!(paths.conflicts.len(), 1);

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_rebase_abort_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("conflict.txt", "base\n");
        repo.git(["add", "conflict.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git(["checkout", "-b", "side"]);
        repo.write("conflict.txt", "side\n");
        repo.git(["commit", "-am", "side"]);
        let side_oid = repo.git_stdout(["rev-parse", "side"]);
        repo.git(["checkout", "main"]);
        repo.write("conflict.txt", "main\n");
        repo.git(["commit", "-am", "main"]);
        repo.git(["checkout", "side"]);
        repo.git_expect_failure(["rebase", "main"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let rebase_head_path = baseline.snapshot.identity.git_dir.join("REBASE_HEAD");
        assert!(matches!(
            baseline.snapshot.operation.kind,
            crate::OperationKind::Rebase
                | crate::OperationKind::RebaseInteractive
                | crate::OperationKind::RebaseMerge
        ));
        assert_eq!(baseline.snapshot.paths.conflicted, vec!["conflict.txt"]);

        repo.git(["rebase", "--abort"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(rebase_head_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Operation,
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.head_changed);
        assert!(delta.delta.operation_changed);
        assert!(delta.delta.branches_changed);
        assert!(delta.delta.paths.has_changes());
        assert_eq!(
            delta.patch.operation.as_ref().unwrap().kind,
            crate::OperationKind::Clean
        );
        assert_eq!(
            delta.patch.head.as_ref().unwrap().oid.as_deref(),
            Some(side_oid.as_str())
        );
        assert_eq!(
            delta.patch.head.as_ref().unwrap().branch.as_deref(),
            Some("side")
        );
        let paths = delta.patch.paths.as_ref().unwrap();
        assert!(paths.conflicted.is_empty());
        assert!(paths.conflicts.is_empty());

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_rebase_continue_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("conflict.txt", "base\n");
        repo.git(["add", "conflict.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git(["checkout", "-b", "side"]);
        repo.write("conflict.txt", "side\n");
        repo.git(["commit", "-am", "side"]);
        let side_before_rebase = repo.git_stdout(["rev-parse", "side"]);
        repo.git(["checkout", "main"]);
        repo.write("conflict.txt", "main\n");
        repo.git(["commit", "-am", "main"]);
        repo.git(["checkout", "side"]);
        repo.git_expect_failure(["rebase", "main"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let rebase_head_path = baseline.snapshot.identity.git_dir.join("REBASE_HEAD");
        assert!(matches!(
            baseline.snapshot.operation.kind,
            crate::OperationKind::Rebase
                | crate::OperationKind::RebaseInteractive
                | crate::OperationKind::RebaseMerge
        ));
        assert_eq!(baseline.snapshot.paths.conflicted, vec!["conflict.txt"]);

        repo.write("conflict.txt", "resolved\n");
        repo.git(["add", "conflict.txt"]);
        repo.git(["rebase", "--continue"]);
        let rebased_oid = repo.git_stdout(["rev-parse", "side"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(rebase_head_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Operation,
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.head_changed);
        assert!(delta.delta.operation_changed);
        assert!(delta.delta.branches_changed);
        assert!(delta.delta.paths.has_changes());
        assert_ne!(rebased_oid, side_before_rebase);
        assert_eq!(
            delta.patch.operation.as_ref().unwrap().kind,
            crate::OperationKind::Clean
        );
        assert_eq!(
            delta.patch.head.as_ref().unwrap().oid.as_deref(),
            Some(rebased_oid.as_str())
        );
        assert_eq!(
            delta.patch.head.as_ref().unwrap().branch.as_deref(),
            Some("side")
        );
        let head = delta
            .patch
            .branches
            .as_ref()
            .unwrap()
            .iter()
            .find(|branch| branch.is_head)
            .unwrap();
        assert_eq!(head.name, "side");
        assert_eq!(head.oid.as_deref(), Some(rebased_oid.as_str()));
        let paths = delta.patch.paths.as_ref().unwrap();
        assert!(paths.conflicted.is_empty());
        assert!(paths.staged.is_empty());
        assert!(paths.unstaged.is_empty());

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_rebase_skip_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("conflict.txt", "base\n");
        repo.git(["add", "conflict.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git(["checkout", "-b", "side"]);
        repo.write("conflict.txt", "side\n");
        repo.git(["commit", "-am", "side"]);
        repo.git(["checkout", "main"]);
        repo.write("conflict.txt", "main\n");
        repo.git(["commit", "-am", "main"]);
        let main_oid = repo.git_stdout(["rev-parse", "main"]);
        repo.git(["checkout", "side"]);
        repo.git_expect_failure(["rebase", "main"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let rebase_head_path = baseline.snapshot.identity.git_dir.join("REBASE_HEAD");
        assert!(matches!(
            baseline.snapshot.operation.kind,
            crate::OperationKind::Rebase
                | crate::OperationKind::RebaseInteractive
                | crate::OperationKind::RebaseMerge
        ));
        assert_eq!(baseline.snapshot.paths.conflicted, vec!["conflict.txt"]);

        repo.git(["rebase", "--skip"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(rebase_head_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Operation,
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.head_changed);
        assert!(delta.delta.operation_changed);
        assert!(delta.delta.branches_changed);
        assert!(delta.delta.paths.has_changes());
        assert_eq!(
            delta.patch.operation.as_ref().unwrap().kind,
            crate::OperationKind::Clean
        );
        assert_eq!(
            delta.patch.head.as_ref().unwrap().oid.as_deref(),
            Some(main_oid.as_str())
        );
        assert_eq!(
            delta.patch.head.as_ref().unwrap().branch.as_deref(),
            Some("side")
        );
        assert!(delta.patch.paths.as_ref().unwrap().conflicted.is_empty());

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_clean_git_cherry_pick_commit_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("base.txt", "base\n");
        repo.git(["add", "base.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git(["checkout", "-b", "side"]);
        repo.write("side.txt", "side\n");
        repo.git(["add", "side.txt"]);
        repo.git(["commit", "-m", "side"]);
        let picked_oid = repo.git_stdout(["rev-parse", "side"]);
        repo.git(["checkout", "main"]);
        repo.write("main.txt", "main\n");
        repo.git(["add", "main.txt"]);
        repo.git(["commit", "-m", "main"]);
        let main_before_pick = repo.git_stdout(["rev-parse", "main"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let main_ref_path = baseline.snapshot.identity.git_dir.join("refs/heads/main");

        repo.git(["cherry-pick", picked_oid.as_str()]);
        let picked_main_oid = repo.git_stdout(["rev-parse", "main"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(main_ref_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.head_changed);
        assert!(delta.delta.branches_changed);
        assert!(!delta.delta.paths.has_changes());
        assert_ne!(picked_main_oid, main_before_pick);
        assert_ne!(picked_main_oid, picked_oid);
        assert_eq!(
            delta.patch.head.as_ref().unwrap().oid.as_deref(),
            Some(picked_main_oid.as_str())
        );
        let head = delta
            .patch
            .branches
            .as_ref()
            .unwrap()
            .iter()
            .find(|branch| branch.is_head)
            .unwrap();
        assert_eq!(head.name, "main");
        assert_eq!(head.oid.as_deref(), Some(picked_main_oid.as_str()));

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_conflicted_git_cherry_pick_commit_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("conflict.txt", "base\n");
        repo.git(["add", "conflict.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git(["checkout", "-b", "side"]);
        repo.write("conflict.txt", "side\n");
        repo.git(["commit", "-am", "side"]);
        let picked_oid = repo.git_stdout(["rev-parse", "side"]);
        repo.git(["checkout", "main"]);
        repo.write("conflict.txt", "main\n");
        repo.git(["commit", "-am", "main"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let cherry_pick_head_path = baseline.snapshot.identity.git_dir.join("CHERRY_PICK_HEAD");

        repo.git_expect_failure(["cherry-pick", picked_oid.as_str()]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(cherry_pick_head_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Operation,
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(!delta.delta.head_changed);
        assert!(delta.delta.operation_changed);
        assert!(delta.delta.paths.has_changes());
        let operation = delta.patch.operation.as_ref().unwrap();
        assert!(matches!(
            operation.kind,
            crate::OperationKind::CherryPick | crate::OperationKind::CherryPickSequence
        ));
        let cherry_pick_head = operation
            .heads
            .iter()
            .find(|head| head.role == crate::OperationHeadRole::CherryPick)
            .unwrap();
        assert_eq!(cherry_pick_head.oid, picked_oid);
        let paths = delta.patch.paths.as_ref().unwrap();
        assert_eq!(paths.conflicted, vec!["conflict.txt"]);
        assert_eq!(paths.conflicts.len(), 1);

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_cherry_pick_abort_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("conflict.txt", "base\n");
        repo.git(["add", "conflict.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git(["checkout", "-b", "side"]);
        repo.write("conflict.txt", "side\n");
        repo.git(["commit", "-am", "side"]);
        let picked_oid = repo.git_stdout(["rev-parse", "side"]);
        repo.git(["checkout", "main"]);
        repo.write("conflict.txt", "main\n");
        repo.git(["commit", "-am", "main"]);
        repo.git_expect_failure(["cherry-pick", picked_oid.as_str()]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let cherry_pick_head_path = baseline.snapshot.identity.git_dir.join("CHERRY_PICK_HEAD");
        assert!(matches!(
            baseline.snapshot.operation.kind,
            crate::OperationKind::CherryPick | crate::OperationKind::CherryPickSequence
        ));
        assert_eq!(baseline.snapshot.paths.conflicted, vec!["conflict.txt"]);

        repo.git(["cherry-pick", "--abort"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(cherry_pick_head_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Operation,
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(!delta.delta.head_changed);
        assert!(delta.delta.operation_changed);
        assert!(delta.delta.paths.has_changes());
        assert_eq!(
            delta.patch.operation.as_ref().unwrap().kind,
            crate::OperationKind::Clean
        );
        let paths = delta.patch.paths.as_ref().unwrap();
        assert!(paths.conflicted.is_empty());
        assert!(paths.conflicts.is_empty());
        assert!(paths.unstaged.is_empty());

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_cherry_pick_continue_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("conflict.txt", "base\n");
        repo.git(["add", "conflict.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git(["checkout", "-b", "side"]);
        repo.write("conflict.txt", "side\n");
        repo.git(["commit", "-am", "side"]);
        let picked_oid = repo.git_stdout(["rev-parse", "side"]);
        repo.git(["checkout", "main"]);
        repo.write("conflict.txt", "main\n");
        repo.git(["commit", "-am", "main"]);
        let main_before_continue = repo.git_stdout(["rev-parse", "main"]);
        repo.git_expect_failure(["cherry-pick", picked_oid.as_str()]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let cherry_pick_head_path = baseline.snapshot.identity.git_dir.join("CHERRY_PICK_HEAD");
        assert!(matches!(
            baseline.snapshot.operation.kind,
            crate::OperationKind::CherryPick | crate::OperationKind::CherryPickSequence
        ));
        assert_eq!(baseline.snapshot.paths.conflicted, vec!["conflict.txt"]);

        repo.write("conflict.txt", "resolved\n");
        repo.git(["add", "conflict.txt"]);
        repo.git(["cherry-pick", "--continue"]);
        let continued_oid = repo.git_stdout(["rev-parse", "main"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(cherry_pick_head_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Operation,
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.head_changed);
        assert!(delta.delta.operation_changed);
        assert!(delta.delta.branches_changed);
        assert!(delta.delta.paths.has_changes());
        assert_ne!(continued_oid, main_before_continue);
        assert_ne!(continued_oid, picked_oid);
        assert_eq!(
            delta.patch.operation.as_ref().unwrap().kind,
            crate::OperationKind::Clean
        );
        assert_eq!(
            delta.patch.head.as_ref().unwrap().oid.as_deref(),
            Some(continued_oid.as_str())
        );
        let head = delta
            .patch
            .branches
            .as_ref()
            .unwrap()
            .iter()
            .find(|branch| branch.is_head)
            .unwrap();
        assert_eq!(head.name, "main");
        assert_eq!(head.oid.as_deref(), Some(continued_oid.as_str()));
        let paths = delta.patch.paths.as_ref().unwrap();
        assert!(paths.conflicted.is_empty());
        assert!(paths.staged.is_empty());
        assert!(paths.unstaged.is_empty());

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_cherry_pick_skip_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("conflict.txt", "base\n");
        repo.git(["add", "conflict.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git(["checkout", "-b", "side"]);
        repo.write("conflict.txt", "side\n");
        repo.git(["commit", "-am", "side"]);
        let picked_oid = repo.git_stdout(["rev-parse", "side"]);
        repo.git(["checkout", "main"]);
        repo.write("conflict.txt", "main\n");
        repo.git(["commit", "-am", "main"]);
        let main_before_skip = repo.git_stdout(["rev-parse", "main"]);
        repo.git_expect_failure(["cherry-pick", picked_oid.as_str()]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let cherry_pick_head_path = baseline.snapshot.identity.git_dir.join("CHERRY_PICK_HEAD");
        assert!(matches!(
            baseline.snapshot.operation.kind,
            crate::OperationKind::CherryPick | crate::OperationKind::CherryPickSequence
        ));
        assert_eq!(baseline.snapshot.paths.conflicted, vec!["conflict.txt"]);

        repo.git(["cherry-pick", "--skip"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(cherry_pick_head_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Operation,
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(!delta.delta.head_changed);
        assert!(delta.delta.operation_changed);
        assert!(delta.delta.paths.has_changes());
        assert!(delta.patch.head.is_none());
        assert_eq!(
            snapshot_repository(repo.path())
                .unwrap()
                .head
                .oid
                .as_deref(),
            Some(main_before_skip.as_str())
        );
        assert_eq!(
            delta.patch.operation.as_ref().unwrap().kind,
            crate::OperationKind::Clean
        );
        assert!(delta.patch.paths.as_ref().unwrap().conflicted.is_empty());

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_clean_git_revert_commit_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.write("tracked.txt", "change\n");
        repo.git(["commit", "-am", "change"]);
        let reverted_oid = repo.git_stdout(["rev-parse", "HEAD"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let main_ref_path = baseline.snapshot.identity.git_dir.join("refs/heads/main");

        repo.git(["revert", reverted_oid.as_str()]);
        let revert_oid = repo.git_stdout(["rev-parse", "main"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(main_ref_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.head_changed);
        assert!(delta.delta.branches_changed);
        assert!(!delta.delta.paths.has_changes());
        assert_ne!(revert_oid, reverted_oid);
        assert_eq!(
            delta.patch.head.as_ref().unwrap().oid.as_deref(),
            Some(revert_oid.as_str())
        );
        let head = delta
            .patch
            .branches
            .as_ref()
            .unwrap()
            .iter()
            .find(|branch| branch.is_head)
            .unwrap();
        assert_eq!(head.name, "main");
        assert_eq!(head.oid.as_deref(), Some(revert_oid.as_str()));

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_conflicted_git_revert_commit_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("conflict.txt", "base\n");
        repo.git(["add", "conflict.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.write("conflict.txt", "reverted\n");
        repo.git(["commit", "-am", "to revert"]);
        let reverted_oid = repo.git_stdout(["rev-parse", "HEAD"]);
        repo.write("conflict.txt", "current\n");
        repo.git(["commit", "-am", "current"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let revert_head_path = baseline.snapshot.identity.git_dir.join("REVERT_HEAD");

        repo.git_expect_failure(["revert", reverted_oid.as_str()]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(revert_head_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Operation,
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(!delta.delta.head_changed);
        assert!(delta.delta.operation_changed);
        assert!(delta.delta.paths.has_changes());
        let operation = delta.patch.operation.as_ref().unwrap();
        assert!(matches!(
            operation.kind,
            crate::OperationKind::Revert | crate::OperationKind::RevertSequence
        ));
        let revert_head = operation
            .heads
            .iter()
            .find(|head| head.role == crate::OperationHeadRole::Revert)
            .unwrap();
        assert_eq!(revert_head.oid, reverted_oid);
        let paths = delta.patch.paths.as_ref().unwrap();
        assert_eq!(paths.conflicted, vec!["conflict.txt"]);
        assert_eq!(paths.conflicts.len(), 1);

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_revert_abort_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("conflict.txt", "base\n");
        repo.git(["add", "conflict.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.write("conflict.txt", "reverted\n");
        repo.git(["commit", "-am", "to revert"]);
        let reverted_oid = repo.git_stdout(["rev-parse", "HEAD"]);
        repo.write("conflict.txt", "current\n");
        repo.git(["commit", "-am", "current"]);
        repo.git_expect_failure(["revert", reverted_oid.as_str()]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let revert_head_path = baseline.snapshot.identity.git_dir.join("REVERT_HEAD");
        assert!(matches!(
            baseline.snapshot.operation.kind,
            crate::OperationKind::Revert | crate::OperationKind::RevertSequence
        ));
        assert_eq!(baseline.snapshot.paths.conflicted, vec!["conflict.txt"]);

        repo.git(["revert", "--abort"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(revert_head_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Operation,
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(!delta.delta.head_changed);
        assert!(delta.delta.operation_changed);
        assert!(delta.delta.paths.has_changes());
        assert_eq!(
            delta.patch.operation.as_ref().unwrap().kind,
            crate::OperationKind::Clean
        );
        let paths = delta.patch.paths.as_ref().unwrap();
        assert!(paths.conflicted.is_empty());
        assert!(paths.conflicts.is_empty());
        assert!(paths.unstaged.is_empty());

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_revert_continue_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("conflict.txt", "base\n");
        repo.git(["add", "conflict.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.write("conflict.txt", "reverted\n");
        repo.git(["commit", "-am", "to revert"]);
        let reverted_oid = repo.git_stdout(["rev-parse", "HEAD"]);
        repo.write("conflict.txt", "current\n");
        repo.git(["commit", "-am", "current"]);
        let main_before_continue = repo.git_stdout(["rev-parse", "main"]);
        repo.git_expect_failure(["revert", reverted_oid.as_str()]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let revert_head_path = baseline.snapshot.identity.git_dir.join("REVERT_HEAD");
        assert!(matches!(
            baseline.snapshot.operation.kind,
            crate::OperationKind::Revert | crate::OperationKind::RevertSequence
        ));
        assert_eq!(baseline.snapshot.paths.conflicted, vec!["conflict.txt"]);

        repo.write("conflict.txt", "resolved\n");
        repo.git(["add", "conflict.txt"]);
        repo.git(["revert", "--continue"]);
        let continued_oid = repo.git_stdout(["rev-parse", "main"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(revert_head_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Operation,
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.head_changed);
        assert!(delta.delta.operation_changed);
        assert!(delta.delta.branches_changed);
        assert!(delta.delta.paths.has_changes());
        assert_ne!(continued_oid, main_before_continue);
        assert_eq!(
            delta.patch.operation.as_ref().unwrap().kind,
            crate::OperationKind::Clean
        );
        assert_eq!(
            delta.patch.head.as_ref().unwrap().oid.as_deref(),
            Some(continued_oid.as_str())
        );
        let head = delta
            .patch
            .branches
            .as_ref()
            .unwrap()
            .iter()
            .find(|branch| branch.is_head)
            .unwrap();
        assert_eq!(head.name, "main");
        assert_eq!(head.oid.as_deref(), Some(continued_oid.as_str()));
        let paths = delta.patch.paths.as_ref().unwrap();
        assert!(paths.conflicted.is_empty());
        assert!(paths.staged.is_empty());
        assert!(paths.unstaged.is_empty());

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_revert_skip_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("conflict.txt", "base\n");
        repo.git(["add", "conflict.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.write("conflict.txt", "reverted\n");
        repo.git(["commit", "-am", "to revert"]);
        let reverted_oid = repo.git_stdout(["rev-parse", "HEAD"]);
        repo.write("conflict.txt", "current\n");
        repo.git(["commit", "-am", "current"]);
        let main_before_skip = repo.git_stdout(["rev-parse", "main"]);
        repo.git_expect_failure(["revert", reverted_oid.as_str()]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let revert_head_path = baseline.snapshot.identity.git_dir.join("REVERT_HEAD");
        assert!(matches!(
            baseline.snapshot.operation.kind,
            crate::OperationKind::Revert | crate::OperationKind::RevertSequence
        ));
        assert_eq!(baseline.snapshot.paths.conflicted, vec!["conflict.txt"]);

        repo.git(["revert", "--skip"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(revert_head_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Operation,
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(!delta.delta.head_changed);
        assert!(delta.delta.operation_changed);
        assert!(delta.delta.paths.has_changes());
        assert!(delta.patch.head.is_none());
        assert_eq!(
            snapshot_repository(repo.path())
                .unwrap()
                .head
                .oid
                .as_deref(),
            Some(main_before_skip.as_str())
        );
        assert_eq!(
            delta.patch.operation.as_ref().unwrap().kind,
            crate::OperationKind::Clean
        );
        assert!(delta.patch.paths.as_ref().unwrap().conflicted.is_empty());

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_bisect_start_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.write("tracked.txt", "second\n");
        repo.git(["commit", "-am", "second"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let bisect_log_path = baseline.snapshot.identity.git_dir.join("BISECT_LOG");

        repo.git(["bisect", "start"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(bisect_log_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Operation,
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(!delta.delta.head_changed);
        assert!(delta.delta.operation_changed);
        assert!(!delta.delta.paths.has_changes());
        let operation = delta.patch.operation.as_ref().unwrap();
        assert_eq!(operation.kind, crate::OperationKind::Bisect);
        assert!(operation.bisect.as_ref().unwrap().good_oids.is_empty());
        assert!(operation.bisect.as_ref().unwrap().bad_oids.is_empty());

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_bisect_bad_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.write("tracked.txt", "second\n");
        repo.git(["commit", "-am", "second"]);
        let bad_oid = repo.git_stdout(["rev-parse", "HEAD"]);
        repo.git(["bisect", "start"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let bisect_log_path = baseline.snapshot.identity.git_dir.join("BISECT_LOG");
        assert!(
            baseline
                .snapshot
                .operation
                .bisect
                .as_ref()
                .unwrap()
                .bad_oids
                .is_empty()
        );

        repo.git(["bisect", "bad"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(bisect_log_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Operation,
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.operation_changed);
        let bisect = delta
            .patch
            .operation
            .as_ref()
            .unwrap()
            .bisect
            .as_ref()
            .unwrap();
        assert_eq!(bisect.bad_oids, vec![bad_oid]);
        assert!(bisect.good_oids.is_empty());

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_bisect_good_commit_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "good\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "good"]);
        let good_oid = repo.git_stdout(["rev-parse", "HEAD"]);
        repo.write("tracked.txt", "middle\n");
        repo.git(["commit", "-am", "middle"]);
        let middle_oid = repo.git_stdout(["rev-parse", "HEAD"]);
        repo.write("tracked.txt", "bad\n");
        repo.git(["commit", "-am", "bad"]);
        let bad_oid = repo.git_stdout(["rev-parse", "HEAD"]);
        repo.git(["bisect", "start"]);
        repo.git(["bisect", "bad"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let bisect_log_path = baseline.snapshot.identity.git_dir.join("BISECT_LOG");

        repo.git(["bisect", "good", good_oid.as_str()]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(bisect_log_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Operation,
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.head_changed);
        assert!(delta.delta.operation_changed);
        assert!(delta.delta.branches_changed);
        assert!(!delta.delta.paths.has_changes());
        assert_eq!(
            delta.patch.head.as_ref().unwrap().oid.as_deref(),
            Some(middle_oid.as_str())
        );
        let bisect = delta
            .patch
            .operation
            .as_ref()
            .unwrap()
            .bisect
            .as_ref()
            .unwrap();
        assert_eq!(bisect.good_oids, vec![good_oid]);
        assert_eq!(bisect.bad_oids, vec![bad_oid]);

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_bisect_reset_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "good\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "good"]);
        let good_oid = repo.git_stdout(["rev-parse", "HEAD"]);
        repo.write("tracked.txt", "middle\n");
        repo.git(["commit", "-am", "middle"]);
        let middle_oid = repo.git_stdout(["rev-parse", "HEAD"]);
        repo.write("tracked.txt", "bad\n");
        repo.git(["commit", "-am", "bad"]);
        let bad_oid = repo.git_stdout(["rev-parse", "HEAD"]);
        repo.git(["bisect", "start"]);
        repo.git(["bisect", "bad"]);
        repo.git(["bisect", "good", good_oid.as_str()]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let bisect_log_path = baseline.snapshot.identity.git_dir.join("BISECT_LOG");
        assert_eq!(
            baseline.snapshot.head.oid.as_deref(),
            Some(middle_oid.as_str())
        );
        assert_eq!(
            baseline.snapshot.operation.kind,
            crate::OperationKind::Bisect
        );

        repo.git(["bisect", "reset"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(bisect_log_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Operation,
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.head_changed);
        assert!(delta.delta.operation_changed);
        assert!(delta.delta.branches_changed);
        assert!(!delta.delta.paths.has_changes());
        assert_eq!(
            delta.patch.operation.as_ref().unwrap().kind,
            crate::OperationKind::Clean
        );
        assert_eq!(
            delta.patch.head.as_ref().unwrap().oid.as_deref(),
            Some(bad_oid.as_str())
        );
        assert_eq!(
            delta.patch.head.as_ref().unwrap().branch.as_deref(),
            Some("main")
        );
        let head = delta
            .patch
            .branches
            .as_ref()
            .unwrap()
            .iter()
            .find(|branch| branch.is_head)
            .unwrap();
        assert_eq!(head.name, "main");
        assert_eq!(head.oid.as_deref(), Some(bad_oid.as_str()));

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_bisect_skip_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "one\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "one"]);
        let good_oid = repo.git_stdout(["rev-parse", "HEAD"]);
        repo.write("tracked.txt", "two\n");
        repo.git(["commit", "-am", "two"]);
        repo.write("tracked.txt", "three\n");
        repo.git(["commit", "-am", "three"]);
        repo.write("tracked.txt", "four\n");
        repo.git(["commit", "-am", "four"]);
        let bad_oid = repo.git_stdout(["rev-parse", "HEAD"]);
        repo.git(["bisect", "start"]);
        repo.git(["bisect", "bad", bad_oid.as_str()]);
        repo.git(["bisect", "good", good_oid.as_str()]);
        let skipped_oid = repo.git_stdout(["rev-parse", "HEAD"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let bisect_log_path = baseline.snapshot.identity.git_dir.join("BISECT_LOG");
        assert_eq!(
            baseline.snapshot.operation.kind,
            crate::OperationKind::Bisect
        );

        repo.git(["bisect", "skip"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(bisect_log_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Operation,
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.head_changed);
        assert!(delta.delta.operation_changed);
        let bisect = delta
            .patch
            .operation
            .as_ref()
            .unwrap()
            .bisect
            .as_ref()
            .unwrap();
        assert!(bisect.skipped_oids.contains(&skipped_oid));

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_worktree_add_branch_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git(["branch", "side"]);
        let linked_parent = TempDir::new().unwrap();
        let linked_path = linked_parent.path().join("linked");
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let worktree_metadata_path = baseline
            .snapshot
            .identity
            .common_dir
            .join("worktrees/linked/gitdir");

        repo.git(["worktree", "add", linked_path.to_str().unwrap(), "side"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(worktree_metadata_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Identity,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Worktrees
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(!delta.delta.identity_changed);
        assert!(delta.delta.worktrees_changed);
        let worktrees = delta.patch.worktrees.as_ref().unwrap();
        assert_eq!(worktrees.len(), 1);
        assert_eq!(worktrees[0].name, "linked");
        assert_eq!(worktrees[0].path, linked_path);
        assert!(!worktrees[0].locked);

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_worktree_add_new_branch_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        let linked_parent = TempDir::new().unwrap();
        let linked_path = linked_parent.path().join("linked");
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let worktree_metadata_path = baseline
            .snapshot
            .identity
            .common_dir
            .join("worktrees/linked/gitdir");

        repo.git([
            "worktree",
            "add",
            "-b",
            "feature",
            linked_path.to_str().unwrap(),
        ]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(worktree_metadata_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Identity,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Worktrees
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(!delta.delta.identity_changed);
        assert!(delta.delta.branches_changed);
        assert!(delta.delta.worktrees_changed);
        assert!(
            delta
                .patch
                .branches
                .as_ref()
                .unwrap()
                .iter()
                .any(|branch| branch.name == "feature")
        );
        let worktrees = delta.patch.worktrees.as_ref().unwrap();
        assert_eq!(worktrees.len(), 1);
        assert_eq!(worktrees[0].name, "linked");
        assert_eq!(worktrees[0].path, linked_path);

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_worktree_remove_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git(["branch", "side"]);
        let linked_parent = TempDir::new().unwrap();
        let linked_path = linked_parent.path().join("linked");
        repo.git(["worktree", "add", linked_path.to_str().unwrap(), "side"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let worktree_metadata_path = baseline
            .snapshot
            .identity
            .common_dir
            .join("worktrees/linked/gitdir");
        assert_eq!(baseline.snapshot.worktrees.len(), 1);

        repo.git(["worktree", "remove", linked_path.to_str().unwrap()]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(worktree_metadata_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Identity,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Worktrees
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(!delta.delta.identity_changed);
        assert!(!delta.delta.branches_changed);
        assert!(delta.delta.worktrees_changed);
        assert!(delta.patch.branches.is_none());
        assert!(delta.patch.worktrees.as_ref().unwrap().is_empty());

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_worktree_prune_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git(["branch", "side"]);
        let linked_parent = TempDir::new().unwrap();
        let linked_path = linked_parent.path().join("linked");
        repo.git(["worktree", "add", linked_path.to_str().unwrap(), "side"]);
        fs::remove_dir_all(&linked_path).unwrap();
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let worktree_metadata_path = baseline
            .snapshot
            .identity
            .common_dir
            .join("worktrees/linked/gitdir");
        assert_eq!(baseline.snapshot.worktrees.len(), 1);

        repo.git(["worktree", "prune"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(worktree_metadata_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Identity,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Worktrees
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.worktrees_changed);
        assert!(delta.patch.worktrees.as_ref().unwrap().is_empty());

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_linked_worktree_branch_commit_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git(["branch", "side"]);
        let linked_parent = TempDir::new().unwrap();
        let linked_path = linked_parent.path().join("linked");
        repo.git(["worktree", "add", linked_path.to_str().unwrap(), "side"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let side_ref_path = baseline
            .snapshot
            .identity
            .common_dir
            .join("refs/heads/side");
        let baseline_side = baseline
            .snapshot
            .branches
            .iter()
            .find(|branch| branch.name == "side")
            .unwrap()
            .oid
            .clone();

        fs::write(linked_path.join("linked.txt"), "linked\n").unwrap();
        git_in(&linked_path, ["add", "linked.txt"]);
        git_in(&linked_path, ["commit", "-m", "linked"]);
        let linked_oid = git_stdout_in(&linked_path, ["rev-parse", "side"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(side_ref_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(!delta.delta.head_changed);
        assert!(delta.delta.branches_changed);
        assert_ne!(baseline_side.as_deref(), Some(linked_oid.as_str()));
        let side = delta
            .patch
            .branches
            .as_ref()
            .unwrap()
            .iter()
            .find(|branch| branch.name == "side")
            .unwrap();
        assert_eq!(side.oid.as_deref(), Some(linked_oid.as_str()));

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_create_local_submodule_repository_emits_no_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);

        let submodule_repo = TestRepo::new();
        submodule_repo.write("README.md", "submodule\n");
        submodule_repo.git(["add", "README.md"]);
        submodule_repo.git(["commit", "-m", "submodule initial"]);

        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(fresh, baseline.snapshot);
    }

    #[test]
    fn command_shape_git_submodule_add_emits_patchable_delta() {
        let submodule_repo = TestRepo::new();
        submodule_repo.write("README.md", "submodule\n");
        submodule_repo.git(["add", "README.md"]);
        submodule_repo.git(["commit", "-m", "submodule initial"]);
        let submodule_url = submodule_repo.path().to_str().unwrap();

        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);

        repo.git_allow_file_protocol(["submodule", "add", submodule_url, "deps/sub"]);
        let (plan, delta) =
            update_from_watch_event(&mut state, event_for(repo.path().join(".gitmodules")));

        assert_eq!(
            plan,
            RefreshPlan::domains([RefreshDomain::Paths, RefreshDomain::Submodules])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.paths.has_changes());
        assert!(delta.delta.submodules_changed);
        assert!(
            delta
                .patch
                .paths
                .as_ref()
                .unwrap()
                .staged
                .contains(&".gitmodules".to_string())
        );
        let submodules = delta.patch.submodules.as_ref().unwrap();
        assert_eq!(submodules.len(), 1);
        let submodule = &submodules[0];
        assert_eq!(submodule.name, "deps/sub");
        assert_eq!(submodule.path, PathBuf::from("deps/sub"));
        assert_eq!(submodule.url.as_deref(), Some(submodule_url));
        assert!(submodule.status.in_config);
        assert!(submodule.status.in_index);
        assert!(submodule.status.in_workdir);
        assert!(submodule.status.index_added);
        assert!(!submodule.status.in_head);

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_submodule_update_init_emits_patchable_delta() {
        let submodule_repo = TestRepo::new();
        submodule_repo.write("README.md", "submodule\n");
        submodule_repo.git(["add", "README.md"]);
        submodule_repo.git(["commit", "-m", "submodule initial"]);
        let submodule_url = submodule_repo.path().to_str().unwrap();

        let super_repo = TestRepo::new();
        super_repo.write("tracked.txt", "base\n");
        super_repo.git(["add", "tracked.txt"]);
        super_repo.git(["commit", "-m", "initial"]);
        super_repo.git_allow_file_protocol(["submodule", "add", submodule_url, "deps/sub"]);
        super_repo.git(["commit", "-am", "add submodule"]);

        let repo = TestRepo::clone_from(super_repo.path());
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let baseline_submodule = baseline
            .snapshot
            .submodules
            .iter()
            .find(|submodule| submodule.name == "deps/sub")
            .unwrap();
        assert!(baseline_submodule.status.workdir_uninitialized);

        repo.git_allow_file_protocol(["submodule", "update", "--init"]);
        let (plan, delta) =
            update_from_watch_event(&mut state, event_for(repo.path().join("deps/sub/.git")));

        assert_eq!(
            plan,
            RefreshPlan::domains([RefreshDomain::Paths, RefreshDomain::Submodules])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.submodules_changed);
        let submodule = delta
            .patch
            .submodules
            .as_ref()
            .unwrap()
            .iter()
            .find(|submodule| submodule.name == "deps/sub")
            .unwrap();
        assert!(submodule.status.in_workdir);
        assert!(!submodule.status.workdir_uninitialized);
        assert!(submodule.workdir_oid.is_some());

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_submodule_commit_emits_parent_patchable_delta() {
        let submodule_repo = TestRepo::new();
        submodule_repo.write("README.md", "submodule\n");
        submodule_repo.git(["add", "README.md"]);
        submodule_repo.git(["commit", "-m", "submodule initial"]);
        let submodule_url = submodule_repo.path().to_str().unwrap();

        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git_allow_file_protocol(["submodule", "add", submodule_url, "deps/sub"]);
        repo.git(["commit", "-am", "add submodule"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let baseline_submodule = baseline
            .snapshot
            .submodules
            .iter()
            .find(|submodule| submodule.name == "deps/sub")
            .unwrap();
        let baseline_workdir_oid = baseline_submodule.workdir_oid.clone();

        let submodule_path = repo.path().join("deps/sub");
        git_in(&submodule_path, ["config", "commit.gpgsign", "false"]);
        git_in(&submodule_path, ["config", "tag.gpgsign", "false"]);
        fs::write(submodule_path.join("README.md"), "submodule changed\n").unwrap();
        git_in(&submodule_path, ["add", "README.md"]);
        git_in(&submodule_path, ["commit", "-m", "submodule change"]);
        let (plan, delta) =
            update_from_watch_event(&mut state, event_for(submodule_path.join("README.md")));

        assert_eq!(
            plan,
            RefreshPlan::domains([RefreshDomain::Paths, RefreshDomain::Submodules])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.paths.has_changes());
        assert!(delta.delta.submodules_changed);
        let submodule = delta
            .patch
            .submodules
            .as_ref()
            .unwrap()
            .iter()
            .find(|submodule| submodule.name == "deps/sub")
            .unwrap();
        assert_ne!(submodule.workdir_oid, baseline_workdir_oid);
        assert!(submodule.status.workdir_modified);

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_submodule_deinit_emits_patchable_delta() {
        let submodule_repo = TestRepo::new();
        submodule_repo.write("README.md", "submodule\n");
        submodule_repo.git(["add", "README.md"]);
        submodule_repo.git(["commit", "-m", "submodule initial"]);
        let submodule_url = submodule_repo.path().to_str().unwrap();

        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git_allow_file_protocol(["submodule", "add", submodule_url, "deps/sub"]);
        repo.git(["commit", "-am", "add submodule"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let baseline_submodule = baseline
            .snapshot
            .submodules
            .iter()
            .find(|submodule| submodule.name == "deps/sub")
            .unwrap();
        assert!(baseline_submodule.status.in_workdir);

        repo.git(["submodule", "deinit", "-f", "deps/sub"]);
        let (plan, delta) =
            update_from_watch_event(&mut state, event_for(repo.path().join("deps/sub")));

        assert_eq!(
            plan,
            RefreshPlan::domains([RefreshDomain::Paths, RefreshDomain::Submodules])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.submodules_changed);
        let submodule = delta
            .patch
            .submodules
            .as_ref()
            .unwrap()
            .iter()
            .find(|submodule| submodule.name == "deps/sub")
            .unwrap();
        assert!(!submodule.status.in_workdir);
        assert!(submodule.status.workdir_uninitialized);

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_submodule_set_url_emits_patchable_delta() {
        let submodule_repo = TestRepo::new();
        submodule_repo.write("README.md", "submodule\n");
        submodule_repo.git(["add", "README.md"]);
        submodule_repo.git(["commit", "-m", "submodule initial"]);
        let replacement_repo = TestRepo::new();
        replacement_repo.write("README.md", "replacement\n");
        replacement_repo.git(["add", "README.md"]);
        replacement_repo.git(["commit", "-m", "replacement initial"]);
        let replacement_url = replacement_repo.path().to_str().unwrap();

        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git_allow_file_protocol([
            "submodule",
            "add",
            submodule_repo.path().to_str().unwrap(),
            "deps/sub",
        ]);
        repo.git(["commit", "-am", "add submodule"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);

        repo.git(["submodule", "set-url", "deps/sub", replacement_url]);
        let (plan, delta) =
            update_from_watch_event(&mut state, event_for(repo.path().join(".gitmodules")));

        assert_eq!(
            plan,
            RefreshPlan::domains([RefreshDomain::Paths, RefreshDomain::Submodules])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.paths.has_changes());
        assert!(delta.delta.submodules_changed);
        let submodule = delta
            .patch
            .submodules
            .as_ref()
            .unwrap()
            .iter()
            .find(|submodule| submodule.name == "deps/sub")
            .unwrap();
        assert_eq!(submodule.url.as_deref(), Some(replacement_url));

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_submodule_set_branch_emits_patchable_delta() {
        let submodule_repo = TestRepo::new();
        submodule_repo.write("README.md", "submodule\n");
        submodule_repo.git(["add", "README.md"]);
        submodule_repo.git(["commit", "-m", "submodule initial"]);
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git_allow_file_protocol([
            "submodule",
            "add",
            submodule_repo.path().to_str().unwrap(),
            "deps/sub",
        ]);
        repo.git(["commit", "-am", "add submodule"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);

        repo.git(["submodule", "set-branch", "--branch", "stable", "deps/sub"]);
        let (plan, delta) =
            update_from_watch_event(&mut state, event_for(repo.path().join(".gitmodules")));

        assert_eq!(
            plan,
            RefreshPlan::domains([RefreshDomain::Paths, RefreshDomain::Submodules])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.paths.has_changes());
        assert!(delta.delta.submodules_changed);
        let submodule = delta
            .patch
            .submodules
            .as_ref()
            .unwrap()
            .iter()
            .find(|submodule| submodule.name == "deps/sub")
            .unwrap();
        assert_eq!(submodule.branch.as_deref(), Some("stable"));

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_submodule_sync_emits_no_semantic_delta() {
        let submodule_repo = TestRepo::new();
        submodule_repo.write("README.md", "submodule\n");
        submodule_repo.git(["add", "README.md"]);
        submodule_repo.git(["commit", "-m", "submodule initial"]);
        let replacement_repo = TestRepo::new();
        replacement_repo.write("README.md", "replacement\n");
        replacement_repo.git(["add", "README.md"]);
        replacement_repo.git(["commit", "-m", "replacement initial"]);
        let replacement_url = replacement_repo.path().to_str().unwrap();

        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git_allow_file_protocol([
            "submodule",
            "add",
            submodule_repo.path().to_str().unwrap(),
            "deps/sub",
        ]);
        repo.git(["commit", "-am", "add submodule"]);
        let gitmodules = repo.path().join(".gitmodules");
        let contents = fs::read_to_string(&gitmodules).unwrap();
        fs::write(
            &gitmodules,
            contents.replace(submodule_repo.path().to_str().unwrap(), replacement_url),
        )
        .unwrap();
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        assert_eq!(
            baseline.snapshot.submodules[0].url.as_deref(),
            Some(replacement_url)
        );
        let config_path = baseline.snapshot.identity.git_dir.join("config");

        repo.git(["submodule", "sync"]);
        let plan = plan_for_watch_event(&state, event_for(config_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Remotes,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths
            ])
        );
        let refresh = refresh_repository_with_plan(
            state.repo(),
            Some(&baseline.snapshot),
            &plan,
            SnapshotOptions::default(),
        )
        .unwrap();
        assert_eq!(refresh.plan, plan);
        assert!(snapshot_update_messages(&mut state, refresh.snapshot).is_empty());
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(fresh, baseline.snapshot);
    }

    #[test]
    fn command_shape_nested_submodule_change_emits_parent_patchable_delta() {
        let nested_repo = TestRepo::new();
        nested_repo.write("README.md", "nested\n");
        nested_repo.git(["add", "README.md"]);
        nested_repo.git(["commit", "-m", "nested initial"]);

        let submodule_repo = TestRepo::new();
        submodule_repo.write("README.md", "submodule\n");
        submodule_repo.git(["add", "README.md"]);
        submodule_repo.git(["commit", "-m", "submodule initial"]);
        submodule_repo.git_allow_file_protocol([
            "submodule",
            "add",
            nested_repo.path().to_str().unwrap(),
            "deps/nested",
        ]);
        submodule_repo.git(["commit", "-am", "add nested submodule"]);

        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git_allow_file_protocol([
            "submodule",
            "add",
            submodule_repo.path().to_str().unwrap(),
            "deps/sub",
        ]);
        repo.git(["commit", "-am", "add submodule"]);
        let submodule_path = repo.path().join("deps/sub");
        git_allow_file_protocol_in(&submodule_path, ["submodule", "update", "--init"]);
        let nested_path = submodule_path.join("deps/nested");
        git_in(&nested_path, ["config", "commit.gpgsign", "false"]);
        git_in(&nested_path, ["config", "tag.gpgsign", "false"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let baseline_submodule = baseline
            .snapshot
            .submodules
            .iter()
            .find(|submodule| submodule.name == "deps/sub")
            .cloned()
            .unwrap();
        assert!(!baseline_submodule.status.workdir_modified);

        fs::write(nested_path.join("README.md"), "nested changed\n").unwrap();
        git_in(&nested_path, ["add", "README.md"]);
        git_in(&nested_path, ["commit", "-m", "nested change"]);
        let (plan, delta) =
            update_from_watch_event(&mut state, event_for(nested_path.join("README.md")));

        assert_eq!(
            plan,
            RefreshPlan::domains([RefreshDomain::Paths, RefreshDomain::Submodules])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.submodules_changed);
        let submodule = delta
            .patch
            .submodules
            .as_ref()
            .unwrap()
            .iter()
            .find(|submodule| submodule.name == "deps/sub")
            .unwrap();
        assert_ne!(submodule, &baseline_submodule);
        assert!(
            submodule.status.workdir_modified
                || submodule.status.workdir_worktree_modified
                || submodule.status.workdir_untracked
        );

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_init_nested_repo_emits_no_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);

        repo.git(["init", "--initial-branch=main", "nested"]);
        let plan = plan_for_watch_event(&state, event_for(repo.path().join("nested/.git/HEAD")));

        assert_eq!(
            plan,
            RefreshPlan::domains([RefreshDomain::Paths, RefreshDomain::Submodules])
        );
        let refresh = refresh_repository_with_plan(
            state.repo(),
            Some(&baseline.snapshot),
            &plan,
            SnapshotOptions::default(),
        )
        .unwrap();
        assert_eq!(refresh.plan, plan);
        assert!(snapshot_update_messages(&mut state, refresh.snapshot).is_empty());
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(fresh, baseline.snapshot);
    }

    #[test]
    fn command_shape_git_init_bare_external_repo_emits_no_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);

        let remote_parent = TempDir::new().unwrap();
        let remote_path = remote_parent.path().join("remote.git");
        init_bare_repo(&remote_path);

        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(fresh, baseline.snapshot);
    }

    #[test]
    fn command_shape_git_config_user_identity_emits_no_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let config_path = baseline.snapshot.identity.git_dir.join("config");

        repo.git(["config", "user.name", "Renamed Tester"]);
        repo.git(["config", "user.email", "renamed@example.com"]);
        let plan = plan_for_watch_event(&state, event_for(config_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Remotes,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths
            ])
        );
        let refresh = refresh_repository_with_plan(
            state.repo(),
            Some(&baseline.snapshot),
            &plan,
            SnapshotOptions::default(),
        )
        .unwrap();
        assert_eq!(refresh.plan, plan);
        assert!(snapshot_update_messages(&mut state, refresh.snapshot).is_empty());
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(fresh, baseline.snapshot);
    }

    #[test]
    fn command_shape_git_commit_allow_empty_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let baseline_oid = baseline.snapshot.head.oid.clone();
        let main_ref_path = baseline
            .snapshot
            .identity
            .common_dir
            .join("refs/heads/main");

        repo.git(["commit", "--allow-empty", "-m", "empty"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(main_ref_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Paths
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.head_changed);
        assert!(delta.delta.branches_changed);
        assert!(!delta.delta.paths.has_changes());
        assert_eq!(delta.patch.paths, None);
        assert_ne!(delta.patch.head.as_ref().unwrap().oid, baseline_oid);
        assert_ne!(
            delta
                .patch
                .head_commit
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap()
                .parent_oids,
            Vec::<String>::new()
        );

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn opening_nested_path_after_directory_creation_uses_repository_snapshot() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        let nested = repo.path().join("nested/deep");
        fs::create_dir_all(&nested).unwrap();

        let mut state = ProcessState::new(&nested);
        let baseline = subscribe_for_deltas(&mut state);
        let fresh = snapshot_repository(repo.path()).unwrap();

        assert_eq!(baseline.snapshot, fresh);
        assert_eq!(state.repo(), nested.as_path());
        assert_eq!(
            baseline.snapshot.identity.worktree_root.as_deref(),
            Some(repo.path())
        );
    }

    #[test]
    fn opening_linked_worktree_path_uses_linked_worktree_snapshot() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git(["branch", "side"]);
        let linked_parent = TempDir::new().unwrap();
        let linked_path = linked_parent.path().join("linked");
        repo.git(["worktree", "add", linked_path.to_str().unwrap(), "side"]);

        let mut state = ProcessState::new(&linked_path);
        let baseline = subscribe_for_deltas(&mut state);
        let fresh = snapshot_repository(&linked_path).unwrap();

        assert_eq!(baseline.snapshot, fresh);
        assert_eq!(state.repo(), linked_path.as_path());
        assert!(baseline.snapshot.identity.is_linked_worktree);
        assert_eq!(
            baseline.snapshot.identity.worktree_root.as_deref(),
            Some(linked_path.as_path())
        );
        assert_eq!(baseline.snapshot.head.branch.as_deref(), Some("side"));
    }

    #[test]
    fn opening_bare_repository_path_uses_bare_snapshot() {
        let bare_parent = TempDir::new().unwrap();
        let bare_path = bare_parent.path().join("repo.git");
        init_bare_repo(&bare_path);

        let mut state = ProcessState::new(&bare_path);
        let baseline = subscribe_for_deltas(&mut state);
        let fresh = snapshot_repository(&bare_path).unwrap();

        assert_eq!(baseline.snapshot, fresh);
        assert_eq!(state.repo(), bare_path.as_path());
        assert!(baseline.snapshot.identity.is_bare);
        assert!(baseline.snapshot.identity.worktree_root.is_none());
        assert!(baseline.snapshot.paths.entries.is_empty());
    }

    #[test]
    fn opening_shallow_clone_path_uses_shallow_snapshot() {
        let remote = TestRepo::new();
        remote.write("tracked.txt", "base\n");
        remote.git(["add", "tracked.txt"]);
        remote.git(["commit", "-m", "initial"]);
        remote.write("tracked.txt", "second\n");
        remote.git(["commit", "-am", "second"]);

        let repo = TestRepo::shallow_clone_from(remote.path());
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let fresh = snapshot_repository(repo.path()).unwrap();

        assert_eq!(baseline.snapshot, fresh);
        assert!(baseline.snapshot.identity.is_shallow);
        assert_eq!(baseline.snapshot.head.branch.as_deref(), Some("main"));
    }

    #[test]
    fn command_shape_git_fetch_deepen_emits_patchable_delta() {
        let remote = TestRepo::new();
        remote.write("tracked.txt", "one\n");
        remote.git(["add", "tracked.txt"]);
        remote.git(["commit", "-m", "one"]);
        remote.write("tracked.txt", "two\n");
        remote.git(["commit", "-am", "two"]);
        remote.write("tracked.txt", "three\n");
        remote.git(["commit", "-am", "three"]);
        let repo = TestRepo::shallow_clone_from(remote.path());
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        assert!(baseline.snapshot.identity.is_shallow);
        let shallow_path = baseline.snapshot.identity.git_dir.join("shallow");

        repo.git_allow_file_protocol(["fetch", "--deepen", "1", "origin"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(shallow_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Identity,
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(!delta.delta.identity_changed);
        assert!(delta.delta.head_changed);
        assert!(delta.delta.branches_changed);
        assert!(!delta.delta.paths.has_changes());
        assert_eq!(delta.patch.identity, None);
        assert_eq!(delta.patch.paths, None);
        assert!(
            !delta
                .patch
                .head_commit
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap()
                .parent_oids
                .is_empty()
        );

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_fetch_unshallow_emits_patchable_delta() {
        let remote = TestRepo::new();
        remote.write("tracked.txt", "one\n");
        remote.git(["add", "tracked.txt"]);
        remote.git(["commit", "-m", "one"]);
        remote.write("tracked.txt", "two\n");
        remote.git(["commit", "-am", "two"]);
        let repo = TestRepo::shallow_clone_from(remote.path());
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        assert!(baseline.snapshot.identity.is_shallow);
        let shallow_path = baseline.snapshot.identity.git_dir.join("shallow");

        repo.git_allow_file_protocol(["fetch", "--unshallow", "origin"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(shallow_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Identity,
                RefreshDomain::Head,
                RefreshDomain::Upstream,
                RefreshDomain::Branches
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.identity_changed);
        assert!(delta.delta.head_changed);
        assert!(delta.delta.branches_changed);
        assert!(!delta.delta.paths.has_changes());
        assert!(!delta.patch.identity.as_ref().unwrap().is_shallow);

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_sparse_checkout_set_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("visible/file.txt", "visible\n");
        repo.write("hidden/file.txt", "hidden\n");
        repo.git(["add", "visible/file.txt", "hidden/file.txt"]);
        repo.git(["commit", "-m", "initial"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let sparse_path = baseline
            .snapshot
            .identity
            .git_dir
            .join("info/sparse-checkout");

        repo.git(["sparse-checkout", "set", "visible"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(sparse_path));

        assert_eq!(plan, RefreshPlan::domains([RefreshDomain::Paths]));
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.paths.has_changes());
        assert!(
            delta
                .delta
                .paths
                .entries_changed
                .contains(&"hidden/file.txt".to_string())
        );
        let hidden = delta
            .patch
            .paths
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .find(|entry| entry.path == "hidden/file.txt")
            .unwrap();
        assert!(hidden.status.skip_worktree);

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_sparse_checkout_disable_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("visible/file.txt", "visible\n");
        repo.write("hidden/file.txt", "hidden\n");
        repo.git(["add", "visible/file.txt", "hidden/file.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git(["sparse-checkout", "set", "visible"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        assert!(
            baseline
                .snapshot
                .paths
                .entries
                .iter()
                .any(|entry| entry.path == "hidden/file.txt" && entry.status.skip_worktree)
        );
        let sparse_path = baseline
            .snapshot
            .identity
            .git_dir
            .join("info/sparse-checkout");

        repo.git(["sparse-checkout", "disable"]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(sparse_path));

        assert_eq!(plan, RefreshPlan::domains([RefreshDomain::Paths]));
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.paths.has_changes());
        let hidden = delta
            .patch
            .paths
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .find(|entry| entry.path == "hidden/file.txt");
        assert!(hidden.is_none_or(|entry| !entry.status.skip_worktree));

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_worktree_lock_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git(["branch", "side"]);
        let linked_parent = TempDir::new().unwrap();
        let linked_path = linked_parent.path().join("linked");
        repo.git(["worktree", "add", linked_path.to_str().unwrap(), "side"]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let lock_path = baseline
            .snapshot
            .identity
            .common_dir
            .join("worktrees/linked/locked");
        assert!(!baseline.snapshot.worktrees[0].locked);

        repo.git([
            "worktree",
            "lock",
            "--reason",
            "testing",
            linked_path.to_str().unwrap(),
        ]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(lock_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Identity,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Worktrees
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.worktrees_changed);
        let worktree = &delta.patch.worktrees.as_ref().unwrap()[0];
        assert!(worktree.locked);
        assert_eq!(worktree.lock_reason.as_deref(), Some("testing"));

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn command_shape_git_worktree_unlock_emits_patchable_delta() {
        let repo = TestRepo::new();
        repo.write("tracked.txt", "base\n");
        repo.git(["add", "tracked.txt"]);
        repo.git(["commit", "-m", "initial"]);
        repo.git(["branch", "side"]);
        let linked_parent = TempDir::new().unwrap();
        let linked_path = linked_parent.path().join("linked");
        repo.git(["worktree", "add", linked_path.to_str().unwrap(), "side"]);
        repo.git([
            "worktree",
            "lock",
            "--reason",
            "testing",
            linked_path.to_str().unwrap(),
        ]);
        let mut state = ProcessState::new(repo.path());
        let baseline = subscribe_for_deltas(&mut state);
        let lock_path = baseline
            .snapshot
            .identity
            .common_dir
            .join("worktrees/linked/locked");
        assert!(baseline.snapshot.worktrees[0].locked);

        repo.git(["worktree", "unlock", linked_path.to_str().unwrap()]);
        let (plan, delta) = update_from_watch_event(&mut state, event_for(lock_path));

        assert_eq!(
            plan,
            RefreshPlan::domains([
                RefreshDomain::Identity,
                RefreshDomain::Upstream,
                RefreshDomain::Branches,
                RefreshDomain::Worktrees
            ])
        );
        assert_eq!(delta.previous_version, baseline.version);
        assert_eq!(delta.version, baseline.version + 1);
        assert!(delta.delta.worktrees_changed);
        let worktree = &delta.patch.worktrees.as_ref().unwrap()[0];
        assert!(!worktree.locked);
        assert!(worktree.lock_reason.is_none());

        let patched = apply_patch_to_snapshot(baseline.snapshot, delta.patch);
        let fresh = snapshot_repository(repo.path()).unwrap();
        assert_eq!(patched, fresh);
    }

    #[test]
    fn rejects_open_and_close_repository_methods() {
        let repo = TestRepo::new();
        let mut state = ProcessState::new(repo.path());

        for method in ["OpenRepository", "CloseRepository"] {
            let messages = handle_request(
                &mut state,
                &format!(r#"{{"jsonrpc":"2.0","id":1,"method":"{method}"}}"#),
            );
            let response = only_response(messages);
            let error = response.error.unwrap();
            assert_eq!(error.code, METHOD_NOT_FOUND);
            assert!(error.message.contains("multi-repository"));
        }
    }

    #[test]
    fn server_messages_round_trip_through_json() {
        let response = ServerMessage::Response(success_response(
            json!("capabilities"),
            json!(Capabilities::current()),
        ));
        let notification = goodbye_message("test shutdown");

        let response_json = serde_json::to_string(&response).unwrap();
        let notification_json = serde_json::to_string(&notification).unwrap();

        assert_eq!(
            serde_json::from_str::<ServerMessage>(&response_json).unwrap(),
            response
        );
        assert_eq!(
            serde_json::from_str::<ServerMessage>(&notification_json).unwrap(),
            notification
        );
    }

    #[test]
    fn goodbye_message_uses_advertised_method_and_reason() {
        let message = goodbye_message("stdin closed");

        match message {
            ServerMessage::Notification(notification) => {
                assert_eq!(notification.method, "gitseer/goodbye");
                assert_eq!(
                    notification.params.unwrap(),
                    json!({ "reason": "stdin closed" })
                );
            }
            ServerMessage::Response(_) => panic!("expected goodbye notification"),
        }
    }

    fn subscribe_for_deltas(state: &mut ProcessState) -> SnapshotNotificationParams {
        let messages = handle_request(
            state,
            r#"{"jsonrpc":"2.0","id":"subscribe","method":"gitseer/subscribe"}"#,
        );
        assert_eq!(messages.len(), 2);
        assert!(matches!(messages[0], ServerMessage::Response(_)));
        match &messages[1] {
            ServerMessage::Notification(notification) => {
                assert_eq!(notification.method, "gitseer/snapshot");
                serde_json::from_value(notification.params.clone().unwrap()).unwrap()
            }
            ServerMessage::Response(_) => panic!("expected snapshot notification"),
        }
    }

    fn update_from_watch_event(
        state: &mut ProcessState,
        event: Event,
    ) -> (RefreshPlan, DeltaNotificationParams) {
        let plan = plan_for_watch_event(state, event);
        let previous = state.latest_snapshot().unwrap().clone();
        let refresh = refresh_repository_with_plan(
            state.repo(),
            Some(&previous),
            &plan,
            SnapshotOptions::default(),
        )
        .unwrap();
        assert_eq!(refresh.plan, plan);
        let messages = snapshot_update_messages(state, refresh.snapshot);
        (plan, only_delta_notification(messages))
    }

    fn plan_for_watch_event(state: &ProcessState, event: Event) -> RefreshPlan {
        let previous = state.latest_snapshot().unwrap();
        refresh_plan_for_event(
            &Ok(event),
            state.repo(),
            previous.identity.worktree_root.as_deref(),
            &previous.identity.git_dir,
            &previous.identity.common_dir,
        )
    }

    fn only_delta_notification(messages: Vec<ServerMessage>) -> DeltaNotificationParams {
        assert_eq!(messages.len(), 1);
        match messages.into_iter().next().unwrap() {
            ServerMessage::Notification(notification) => {
                assert_eq!(notification.method, "gitseer/delta");
                serde_json::from_value(notification.params.unwrap()).unwrap()
            }
            ServerMessage::Response(_) => panic!("expected delta notification"),
        }
    }

    fn apply_patch_to_snapshot(
        mut snapshot: RepositorySnapshot,
        patch: SnapshotPatch,
    ) -> RepositorySnapshot {
        if let Some(identity) = patch.identity {
            snapshot.identity = identity;
        }
        if let Some(head) = patch.head {
            snapshot.head = head;
        }
        if let Some(head_commit) = patch.head_commit {
            snapshot.head_commit = head_commit;
        }
        if let Some(upstream) = patch.upstream {
            snapshot.upstream = upstream;
        }
        if let Some(paths) = patch.paths {
            snapshot.paths = paths;
        }
        if let Some(operation) = patch.operation {
            snapshot.operation = operation;
        }
        if let Some(remotes) = patch.remotes {
            snapshot.remotes = remotes;
        }
        if let Some(branches) = patch.branches {
            snapshot.branches = branches;
        }
        if let Some(tags) = patch.tags {
            snapshot.tags = tags;
        }
        if let Some(stashes) = patch.stashes {
            snapshot.stashes = stashes;
        }
        if let Some(worktrees) = patch.worktrees {
            snapshot.worktrees = worktrees;
        }
        if let Some(submodules) = patch.submodules {
            snapshot.submodules = submodules;
        }
        snapshot
    }

    fn event_for(path: impl Into<PathBuf>) -> Event {
        let mut event = Event::new(EventKind::Any);
        event.paths.push(path.into());
        event
    }

    fn init_bare_repo(path: &Path) {
        let output = Command::new("git")
            .args(["init", "--bare", path.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git command failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_in<const N: usize>(path: &Path, args: [&str; N]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git command failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_allow_file_protocol_in<const N: usize>(path: &Path, args: [&str; N]) {
        let output = Command::new("git")
            .arg("-c")
            .arg("protocol.file.allow=always")
            .args(args)
            .current_dir(path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git command failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_stdout_in<const N: usize>(path: &Path, args: [&str; N]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git command failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    fn only_response(messages: Vec<ServerMessage>) -> JsonRpcResponse {
        assert_eq!(messages.len(), 1);
        match messages.into_iter().next().unwrap() {
            ServerMessage::Response(response) => response,
            ServerMessage::Notification(_) => panic!("expected response"),
        }
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
            repo.git(["config", "core.editor", "true"]);
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
            repo.git(["config", "core.editor", "true"]);
            repo
        }

        fn shallow_clone_from(remote: &Path) -> Self {
            let temp = TempDir::new().unwrap();
            let remote_url = format!("file://{}", remote.to_string_lossy());
            let output = Command::new("git")
                .arg("-c")
                .arg("protocol.file.allow=always")
                .args(["clone", "--depth", "1", &remote_url])
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
            repo.git(["config", "core.editor", "true"]);
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

        fn git<const N: usize>(&self, args: [&str; N]) {
            let output = Command::new("git")
                .args(args)
                .current_dir(self.path())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git command failed\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }

        fn git_allow_file_protocol<const N: usize>(&self, args: [&str; N]) {
            let output = Command::new("git")
                .arg("-c")
                .arg("protocol.file.allow=always")
                .args(args)
                .current_dir(self.path())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git command failed\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }

        fn git_expect_failure<const N: usize>(&self, args: [&str; N]) {
            let output = Command::new("git")
                .args(args)
                .current_dir(self.path())
                .output()
                .unwrap();
            assert!(
                !output.status.success(),
                "git command unexpectedly succeeded\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }

        fn git_stdout<const N: usize>(&self, args: [&str; N]) -> String {
            let output = Command::new("git")
                .args(args)
                .current_dir(self.path())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git command failed\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap().trim().to_string()
        }
    }
}
