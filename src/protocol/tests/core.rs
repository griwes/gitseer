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
