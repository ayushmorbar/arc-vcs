use anyhow::Context as _;
use arc_cli::repo::Repository;
use arc_core::algebra::Blake3Hash;
use arc_core::store::author::load_identity;
use arc_core::store::oplog::OpLog;
use arc_core::store::oplog::Operation;
use arc_core::store::view::View;
use serde::Serialize;
use serde_json::json;
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::protocol::{RpcRequest, RpcResponse};

#[derive(Serialize)]
struct StatusResult {
    current_view: String,
    current_view_hash: Option<String>,
    has_conflicts: bool,
}

#[derive(Serialize)]
struct OplogEntry {
    action: String,
    timestamp: u64,
    view_hash: Option<String>,
}

#[derive(serde::Deserialize)]
struct PathParams {
    path: String,
}

enum RpcDispatchError {
    MethodNotFound(String),
    InvalidParams(String),
    Internal(anyhow::Error),
}

impl RpcDispatchError {
    fn code(&self) -> i64 {
        match self {
            Self::MethodNotFound(_) => -32601,
            Self::InvalidParams(_) => -32602,
            Self::Internal(_) => -32603,
        }
    }

    fn message(&self) -> String {
        match self {
            Self::MethodNotFound(msg) => msg.clone(),
            Self::InvalidParams(msg) => msg.clone(),
            Self::Internal(err) => err.to_string(),
        }
    }
}

/// Run the JSON-RPC 2.0 server loop over stdin/stdout.
pub async fn run() -> anyhow::Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = BufReader::new(stdin).lines();
    let mut writer = io::BufWriter::new(stdout);

    while let Some(line) = reader.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<RpcRequest>(&line) {
            Ok(req) => handle_request(req).await,
            Err(err) => RpcResponse::err(0, -32700, format!("parse error: {err}")),
        };

        let payload = serde_json::to_string(&response)?;
        writer.write_all(payload.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
    }

    Ok(())
}

async fn handle_request(request: RpcRequest) -> RpcResponse<serde_json::Value> {
    if request.jsonrpc != "2.0" {
        return RpcResponse::err(request.id, -32600, "invalid request: jsonrpc must be '2.0'");
    }

    let result = match request.method.as_str() {
        "get_status" => get_status(request.params).await.map(|r| json!(r)),
        "get_oplog" => get_oplog(request.params).await.map(|r| json!(r)),
        _ => Err(RpcDispatchError::MethodNotFound(format!(
            "method '{}' is not implemented",
            request.method
        ))),
    };

    match result {
        Ok(data) => RpcResponse::ok(request.id, data),
        Err(err) => RpcResponse::err(request.id, err.code(), err.message()),
    }
}

async fn get_status(params: Option<serde_json::Value>) -> Result<StatusResult, RpcDispatchError> {
    let path = parse_path(params)?;
    let join = tokio::task::spawn_blocking(move || -> anyhow::Result<StatusResult> {
        let mut repo = Repository::open(&path)?;
        if let Ok((author, signing_key)) = load_identity() {
            repo.set_identity(author, signing_key);
        }

        repo.snapshot()?;

        let current_view = repo.current_view_name()?;
        let view = View::load(&repo.shared_root, &current_view)
            .map_err(|e| anyhow::anyhow!("failed to load current view: {e}"))?;

        let current_view_hash = select_head_hash(&view.heads);
        let has_conflicts = repo.shared_root.join(".arc").join("conflict").exists();

        Ok(StatusResult {
            current_view,
            current_view_hash,
            has_conflicts,
        })
    })
    .await
    .map_err(|e| RpcDispatchError::Internal(anyhow::anyhow!("status task join error: {e}")))?;

    join.map_err(RpcDispatchError::Internal)
}

async fn get_oplog(params: Option<serde_json::Value>) -> Result<Vec<OplogEntry>, RpcDispatchError> {
    let path = parse_path(params)?;
    let join = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<OplogEntry>> {
        let repo = Repository::open(&path)?;
        let arc_dir = repo.shared_root.join(".arc");
        let oplog_dir = arc_dir.join("oplog");
        let entries = if oplog_dir.is_dir() {
            read_oplog_directory(&oplog_dir)?
        } else {
            let oplog = OpLog::new(&arc_dir);
            oplog.read_reversed()?
        };

        let result = entries
            .into_iter()
            .take(10)
            .map(|entry| {
                let view_hash = if !entry.after_heads.is_empty() {
                    select_head_hash(&entry.after_heads)
                } else {
                    select_head_hash(&entry.before_heads)
                };
                OplogEntry {
                    action: entry.command,
                    timestamp: entry.timestamp,
                    view_hash,
                }
            })
            .collect();

        Ok(result)
    })
    .await
    .map_err(|e| RpcDispatchError::Internal(anyhow::anyhow!("oplog task join error: {e}")))?;

    join.map_err(RpcDispatchError::Internal)
}

fn read_oplog_directory(oplog_dir: &std::path::Path) -> anyhow::Result<Vec<Operation>> {
    let mut entries = Vec::new();
    for item in std::fs::read_dir(oplog_dir)? {
        let entry = item?;
        let file_type = entry.file_type()?;
        if !file_type.is_file() {
            continue;
        }

        let data = std::fs::read_to_string(entry.path())?;
        let op: Operation = serde_json::from_str(&data)
            .with_context(|| format!("failed to parse oplog entry {}", entry.path().display()))?;
        entries.push(op);
    }

    entries.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    Ok(entries)
}

fn parse_path(params: Option<serde_json::Value>) -> Result<std::path::PathBuf, RpcDispatchError> {
    let parsed: PathParams = serde_json::from_value(params.unwrap_or_default()).map_err(|_| {
        RpcDispatchError::InvalidParams(
            "missing or invalid params: expected {\"path\": \"...\"}".to_string(),
        )
    })?;
    Ok(std::path::PathBuf::from(parsed.path))
}

fn select_head_hash(heads: &std::collections::HashSet<Blake3Hash>) -> Option<String> {
    let mut hashes: Vec<String> = heads.iter().map(hash_to_hex).collect();
    hashes.sort();
    hashes.into_iter().next()
}

fn hash_to_hex(hash: &Blake3Hash) -> String {
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn get_status_rpc_response_has_expected_shape() {
        let dir = tempfile::tempdir().unwrap();
        Repository::init(dir.path()).unwrap();

        let req = RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: 7,
            method: "get_status".to_string(),
            params: Some(json!({ "path": dir.path().display().to_string() })),
        };

        let resp = handle_request(req).await;
        let value = serde_json::to_value(&resp).unwrap();

        assert_eq!(value["jsonrpc"], "2.0");
        assert_eq!(value["id"], 7);
        assert!(value["error"].is_null(), "unexpected error: {value}");

        let result = &value["result"];
        assert!(result["current_view"].is_string());
        assert!(result["has_conflicts"].is_boolean());
        assert!(result["current_view_hash"].is_string() || result["current_view_hash"].is_null());
    }
}
