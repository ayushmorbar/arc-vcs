use serde::{Deserialize, Serialize};
use std::io::Write;
use std::sync::{Mutex, OnceLock};

/// Incoming JSON-RPC 2.0 request.
#[derive(Debug, Deserialize)]
pub struct RpcRequest {
    /// JSON-RPC protocol version.
    pub jsonrpc: String,
    /// Request id echoed in the response.
    ///
    /// Batch 1 intentionally supports numeric ids only.
    pub id: u64,
    /// Method name to dispatch.
    pub method: String,
    /// Optional method-specific parameters.
    pub params: Option<serde_json::Value>,
}

/// Outgoing JSON-RPC 2.0 response.
#[derive(Debug, Serialize)]
pub struct RpcResponse<T>
where
    T: Serialize,
{
    /// JSON-RPC protocol version.
    pub jsonrpc: String,
    /// Request id echoed from the request.
    pub id: u64,
    /// Successful result payload.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<T>,
    /// Error payload when the request fails.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl<T> RpcResponse<T>
where
    T: Serialize,
{
    /// Construct a successful response.
    pub fn ok(id: u64, result: T) -> Self {
        Self { jsonrpc: "2.0".to_string(), id, result: Some(result), error: None }
    }

    /// Construct an error response.
    pub fn err(id: u64, code: i64, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(RpcError { code, message: message.into() }),
        }
    }
}

/// JSON-RPC error object.
#[derive(Debug, Serialize)]
pub struct RpcError {
    /// JSON-RPC error code.
    pub code: i64,
    /// Human-readable error message.
    pub message: String,
}

/// Outgoing JSON-RPC 2.0 notification payload.
#[derive(Debug, Serialize)]
pub struct RpcNotification<T>
where
    T: Serialize,
{
    /// JSON-RPC protocol version.
    pub jsonrpc: String,
    /// Notification method name.
    pub method: String,
    /// Optional notification parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<T>,
}

/// Parameters for the `get_file_states` RPC method.
#[derive(Debug, Deserialize)]
pub struct GetFileStatesParams {
    /// Repository path.
    pub path: String,
}

/// IDE decoration state for one file.
#[derive(Debug, Clone, Serialize)]
pub struct FileState {
    /// Repository-relative file path.
    pub file_path: String,
    /// Decoration status: `modified`, `untracked`, `conflict`, or `ai_generated`.
    pub status: String,
}

/// Serialize and emit a JSON-RPC response as one stdout line.
pub fn send_response<T>(response: &RpcResponse<T>) -> anyhow::Result<()>
where
    T: Serialize,
{
    write_json_line(response)
}

/// Serialize and emit a JSON-RPC notification as one stdout line.
pub fn send_notification<T: Serialize>(method: &str, params: Option<T>) -> anyhow::Result<()> {
    let payload =
        RpcNotification { jsonrpc: "2.0".to_string(), method: method.to_string(), params };
    write_json_line(&payload)
}

fn write_json_line<T: Serialize>(value: &T) -> anyhow::Result<()> {
    static STDOUT_LOCK: OnceLock<Mutex<std::io::Stdout>> = OnceLock::new();
    let lock = STDOUT_LOCK.get_or_init(|| Mutex::new(std::io::stdout()));

    let mut stdout = lock.lock().map_err(|_| anyhow::anyhow!("stdout lock was poisoned"))?;
    let payload = serde_json::to_string(value)?;
    stdout.write_all(payload.as_bytes())?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_serialization_has_no_id_field() {
        let notification = RpcNotification {
            jsonrpc: "2.0".to_string(),
            method: "arc/stateChanged".to_string(),
            params: Option::<serde_json::Value>::None,
        };

        let value = serde_json::to_value(notification).expect("serialization should succeed");
        assert_eq!(value["jsonrpc"], "2.0");
        assert_eq!(value["method"], "arc/stateChanged");
        assert!(value.get("id").is_none());
    }
}
