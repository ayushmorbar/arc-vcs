use arc_daemon::protocol::*;

#[test]
fn rpc_request_deserialize_minimal() {
    let json = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
    let req: RpcRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.jsonrpc, "2.0");
    assert_eq!(req.id, 1);
    assert_eq!(req.method, "ping");
    assert!(req.params.is_none());
}

#[test]
fn rpc_request_deserialize_with_params() {
    let json = r#"{"jsonrpc":"2.0","id":42,"method":"get_file_states","params":{"path":"/repo"}}"#;
    let req: RpcRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.id, 42);
    assert_eq!(req.method, "get_file_states");
    let params = req.params.unwrap();
    assert_eq!(params["path"], "/repo");
}

#[test]
fn rpc_request_deserialize_string_id() {
    let json = r#"{"jsonrpc":"2.0","id":"abc","method":"test"}"#;
    let result = serde_json::from_str::<RpcRequest>(json);
    // Batch 1 only supports numeric ids; string id should fail
    assert!(result.is_err());
}

#[test]
fn rpc_response_ok_minimal() {
    let resp = RpcResponse::ok(1, "hello");
    let json = serde_json::to_value(&resp).unwrap();
    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 1);
    assert_eq!(json["result"], "hello");
    assert!(json.get("error").is_none());
}

#[test]
fn rpc_response_ok_with_struct() {
    #[derive(serde::Serialize)]
    struct Info {
        name: String,
        count: u32,
    }
    let resp = RpcResponse::ok(5, Info { name: "test".into(), count: 3 });
    let json = serde_json::to_value(&resp).unwrap();
    assert_eq!(json["result"]["name"], "test");
    assert_eq!(json["result"]["count"], 3);
}

#[test]
fn rpc_response_err() {
    let resp: RpcResponse<()> = RpcResponse::err(10, -32600, "Invalid Request");
    let json = serde_json::to_value(&resp).unwrap();
    assert!(json.get("result").is_none());
    let err = json.get("error").unwrap();
    assert_eq!(err["code"], -32600);
    assert_eq!(err["message"], "Invalid Request");
}

#[test]
fn rpc_response_err_message_into() {
    let resp: RpcResponse<()> = RpcResponse::err(1, -1, String::from("owned string"));
    let json = serde_json::to_value(&resp).unwrap();
    assert_eq!(json["error"]["message"], "owned string");
}

#[test]
fn notification_serialization_no_id() {
    let notification = RpcNotification {
        jsonrpc: "2.0".to_string(),
        method: "arc/stateChanged".to_string(),
        params: Option::<serde_json::Value>::None,
    };
    let json = serde_json::to_value(&notification).unwrap();
    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["method"], "arc/stateChanged");
    assert!(json.get("id").is_none());
}

#[test]
fn notification_with_params() {
    #[derive(serde::Serialize)]
    struct State {
        status: String,
    }
    let notification = RpcNotification {
        jsonrpc: "2.0".to_string(),
        method: "arc/progress".to_string(),
        params: Some(State { status: "running".into() }),
    };
    let json = serde_json::to_value(&notification).unwrap();
    assert_eq!(json["params"]["status"], "running");
}

#[test]
fn notification_params_none_skips_field() {
    let notification = RpcNotification {
        jsonrpc: "2.0".to_string(),
        method: "arc/heartbeat".to_string(),
        params: Option::<serde_json::Value>::None,
    };
    let json = serde_json::to_value(&notification).unwrap();
    assert!(json.get("params").is_none());
}

#[test]
fn file_state_serialize() {
    let state = FileState { file_path: "src/main.rs".into(), status: "modified".into() };
    let json = serde_json::to_value(&state).unwrap();
    assert_eq!(json["file_path"], "src/main.rs");
    assert_eq!(json["status"], "modified");
}

#[test]
fn file_state_statuses() {
    for status in &["modified", "untracked", "conflict", "ai_generated"] {
        let state = FileState { file_path: "test.rs".into(), status: status.to_string() };
        let json = serde_json::to_value(&state).unwrap();
        assert_eq!(json["status"], *status);
    }
}

#[test]
fn get_file_states_params_deserialize() {
    let json = r#"{"path":"/repo"}"#;
    let params: GetFileStatesParams = serde_json::from_str(json).unwrap();
    assert_eq!(params.path, "/repo");
}

#[test]
fn rpc_error_fields() {
    let err = RpcError { code: -32601, message: "Method not found".into() };
    let json = serde_json::to_value(&err).unwrap();
    assert_eq!(json["code"], -32601);
    assert_eq!(json["message"], "Method not found");
}

#[test]
fn rpc_response_debug_format() {
    let resp = RpcResponse::ok(1, "test");
    let dbg = format!("{resp:?}");
    assert!(dbg.contains("RpcResponse"));
}

#[test]
fn rpc_request_debug_format() {
    let req = RpcRequest { jsonrpc: "2.0".into(), id: 1, method: "ping".into(), params: None };
    let dbg = format!("{req:?}");
    assert!(dbg.contains("RpcRequest"));
}
