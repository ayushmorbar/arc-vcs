use serde::{Deserialize, Serialize};

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
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    /// Construct an error response.
    pub fn err(id: u64, code: i64, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(RpcError {
                code,
                message: message.into(),
            }),
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
