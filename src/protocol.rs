use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    Capabilities, RepositorySnapshot, SnapshotDelta, SnapshotOptions, SnapshotPatch,
    snapshot_delta, snapshot_repository, snapshot_repository_with_options,
};

const JSONRPC_VERSION: &str = "2.0";
const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
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

    let request_object = match request_value.as_object() {
        Some(object) => object,
        None => {
            return vec![ServerMessage::Response(error_response(
                Value::Null,
                INVALID_REQUEST,
                "invalid request",
                Some("JSON-RPC requests must be objects".to_string()),
            ))];
        }
    };
    let is_notification = !request_object.contains_key("id");
    let response_id = match request_object.get("id") {
        Some(id) if !valid_request_id(id) => Value::Null,
        Some(id) => id.clone(),
        None => Value::Null,
    };

    if request_object.get("jsonrpc").and_then(Value::as_str) != Some(JSONRPC_VERSION) {
        return invalid_request_response(response_id, "jsonrpc must be exactly \"2.0\"");
    }
    if !request_object.get("method").is_some_and(Value::is_string) {
        return invalid_request_response(response_id, "method must be a string");
    }
    if request_object
        .get("id")
        .is_some_and(|id| !valid_request_id(id))
    {
        return invalid_request_response(Value::Null, "id must be a string, number, or null");
    }
    if request_object
        .get("params")
        .is_some_and(|params| !params.is_null() && !params.is_object() && !params.is_array())
    {
        return invalid_request_response(response_id, "params must be an object, array, or null");
    }

    let request = match serde_json::from_value::<JsonRpcRequest>(request_value) {
        Ok(request) => request,
        Err(error) => {
            return invalid_request_response(response_id, &error.to_string());
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

fn valid_request_id(id: &Value) -> bool {
    id.is_null() || id.is_string() || id.is_number()
}

fn invalid_request_response(id: Value, detail: &str) -> Vec<ServerMessage> {
    vec![ServerMessage::Response(error_response(
        id,
        INVALID_REQUEST,
        "invalid request",
        Some(detail.to_string()),
    ))]
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
mod tests;
