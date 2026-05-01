use super::*;

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
