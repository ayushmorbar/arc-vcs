use anyhow::Context as _;
use arc_cli::repo::Repository;
use arc_core::algebra::{Atom, Blake3Hash};
use arc_core::store::author::Author;
use arc_core::store::author::load_identity;
use arc_core::store::newtypes::ChangeId;
use arc_core::store::oplog::OpLog;
use arc_core::store::view::View;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use serde_json::json;
use tokio::io::{self, AsyncBufReadExt, BufReader};
use tokio::time::{Duration, timeout};

use crate::protocol::{
    FileState, GetFileStatesParams, RpcRequest, RpcResponse, send_notification, send_response,
};

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
    if let Ok(repo) = Repository::open(".") {
        let work_root = repo.work_root.clone();
        let arc_dir = repo.shared_root.join(".arc");
        tokio::spawn(async move {
            if let Err(err) = spawn_repo_watcher(work_root, arc_dir).await {
                eprintln!("[arc-daemon] watcher stopped: {err}");
            }
        });
    } else {
        eprintln!("[arc-daemon] repository not found in current directory; watcher disabled");
    }

    let stdin = io::stdin();
    let mut reader = BufReader::new(stdin).lines();

    while let Some(line) = reader.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<RpcRequest>(&line) {
            Ok(req) => handle_request(req).await,
            Err(err) => RpcResponse::err(0, -32700, format!("parse error: {err}")),
        };

        send_response(&response)?;
    }

    Ok(())
}

async fn spawn_repo_watcher(
    work_root: std::path::PathBuf,
    arc_dir: std::path::PathBuf,
) -> anyhow::Result<()> {
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<()>(1);
    let callback_tx = event_tx.clone();

    let mut watcher = RecommendedWatcher::new(
        move |event: notify::Result<notify::Event>| match event {
            Ok(_) => {
                // Coalescing signal: if one event is already pending, drop extras.
                let _ = callback_tx.try_send(());
            }
            Err(err) => {
                eprintln!("[arc-daemon] watcher event error: {err}");
            }
        },
        notify::Config::default(),
    )
    .context("failed to create filesystem watcher")?;

    watcher
        .watch(&work_root, RecursiveMode::Recursive)
        .with_context(|| format!("failed to watch {}", work_root.display()))?;

    if arc_dir.exists() && arc_dir != work_root {
        watcher
            .watch(&arc_dir, RecursiveMode::Recursive)
            .with_context(|| format!("failed to watch {}", arc_dir.display()))?;
    }

    while event_rx.recv().await.is_some() {
        loop {
            match timeout(Duration::from_millis(100), event_rx.recv()).await {
                Ok(Some(_)) => continue,
                Ok(None) => return Ok(()),
                Err(_) => break,
            }
        }

        if let Err(err) = send_notification("arc/stateChanged", None::<serde_json::Value>) {
            eprintln!("[arc-daemon] failed to send notification: {err}");
        }
        if let Err(err) = send_notification("arc/fileDecorationsChanged", None::<serde_json::Value>)
        {
            eprintln!("[arc-daemon] failed to send notification: {err}");
        }
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
        "get_file_states" => get_file_states(request.params).await.map(|r| json!(r)),
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
        let oplog = OpLog::new(&arc_dir);
        let entries = oplog.read_reversed()?;

        let result = entries
            .into_iter()
            .take(10)
            .map(|entry| {
                let view_hash = if !entry.after_heads.is_empty() {
                    select_change_head_hash(&entry.after_heads)
                } else {
                    select_change_head_hash(&entry.before_heads)
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

async fn get_file_states(
    params: Option<serde_json::Value>,
) -> Result<Vec<FileState>, RpcDispatchError> {
    let path = parse_path(params)?;
    let join = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<FileState>> {
        let mut repo = Repository::open(&path)?;
        let view_name = repo.current_view_name()?;

        repo.hydrate(&view_name)?;

        let materialized = repo.materialize(&view_name)?;
        let tracked_files = tracked_files_from_state(&materialized);
        let delta = repo.status()?;
        let history = repo.log()?;

        let mut statuses: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();

        for atom in &delta {
            if let Some(file_path) = file_path_from_atom(atom) {
                let status = if tracked_files.contains(&file_path) {
                    "modified"
                } else {
                    "untracked"
                };
                upsert_status(&mut statuses, &file_path, status);
            }
        }

        let (conflict_files, ai_generated_files) = file_attribution_from_history(&history);
        for file_path in conflict_files {
            upsert_status(&mut statuses, &file_path, "conflict");
        }
        for file_path in ai_generated_files {
            upsert_status(&mut statuses, &file_path, "ai_generated");
        }

        let mut out: Vec<FileState> = statuses
            .into_iter()
            .map(|(file_path, status)| FileState { file_path, status })
            .collect();
        out.sort_by(|a, b| a.file_path.cmp(&b.file_path));
        Ok(out)
    })
    .await
    .map_err(|e| RpcDispatchError::Internal(anyhow::anyhow!("file_states task join error: {e}")))?;

    join.map_err(RpcDispatchError::Internal)
}

fn parse_path(params: Option<serde_json::Value>) -> Result<std::path::PathBuf, RpcDispatchError> {
    let parsed: GetFileStatesParams =
        serde_json::from_value(params.unwrap_or_default()).map_err(|_| {
            RpcDispatchError::InvalidParams(
                "missing or invalid params: expected {\"path\": \"...\"}".to_string(),
            )
        })?;
    Ok(std::path::PathBuf::from(parsed.path))
}

fn tracked_files_from_state(
    state: &arc_core::algebra::apply::MaterializedState,
) -> std::collections::HashSet<String> {
    let mut tracked = std::collections::HashSet::new();
    for key in state.keys() {
        if key.len() >= 2 && key[0] == "file" {
            tracked.insert(key[1].clone());
        }
    }
    tracked
}

fn file_path_from_atom(atom: &Atom) -> Option<String> {
    match atom {
        Atom::Insert { at, .. }
        | Atom::Delete { at, .. }
        | Atom::SemanticsPreserving { at, .. }
        | Atom::Conflict { at, .. }
            if at.len() >= 2 && at[0] == "file" =>
        {
            Some(at[1].clone())
        }
        Atom::Blob { path, .. } | Atom::Mount { path, .. } | Atom::Directory { path }
            if path.len() >= 2 && path[0] == "file" =>
        {
            Some(path[1].clone())
        }
        Atom::Move { to, .. } if to.len() >= 2 && to[0] == "file" => Some(to[1].clone()),
        _ => None,
    }
}

fn upsert_status(map: &mut std::collections::HashMap<String, String>, file: &str, status: &str) {
    fn precedence(status: &str) -> u8 {
        match status {
            "modified" => 1,
            "untracked" => 2,
            "ai_generated" => 3,
            "conflict" => 4,
            _ => 0,
        }
    }

    match map.get(file) {
        Some(existing) if precedence(existing) >= precedence(status) => {}
        _ => {
            map.insert(file.to_string(), status.to_string());
        }
    }
}

fn file_attribution_from_history(
    history_newest_first: &[arc_core::store::change::Change],
) -> (
    std::collections::HashSet<String>,
    std::collections::HashSet<String>,
) {
    let mut conflict_files = std::collections::HashSet::new();
    let mut ai_generated_files = std::collections::HashSet::new();

    // Replay oldest -> newest to model the last-writer projection.
    for change in history_newest_first.iter().rev() {
        for atom in &change.atoms {
            let Some(file_path) = file_path_from_atom(atom) else {
                continue;
            };

            if matches!(atom, Atom::Conflict { .. }) {
                conflict_files.insert(file_path.clone());
                ai_generated_files.remove(&file_path);
                continue;
            }

            conflict_files.remove(&file_path);
            if matches!(&change.author, Author::AI { .. }) {
                ai_generated_files.insert(file_path);
            } else {
                ai_generated_files.remove(&file_path);
            }
        }
    }

    (conflict_files, ai_generated_files)
}

fn select_head_hash(heads: &std::collections::HashSet<Blake3Hash>) -> Option<String> {
    let mut hashes: Vec<String> = heads.iter().map(hash_to_hex).collect();
    hashes.sort();
    hashes.into_iter().next()
}

fn select_change_head_hash(heads: &std::collections::BTreeSet<ChangeId>) -> Option<String> {
    let mut hashes: Vec<String> = heads.iter().map(|id| hash_to_hex(&id.0)).collect();
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

    #[tokio::test]
    async fn get_file_states_rpc_response_has_expected_shape() {
        let dir = tempfile::tempdir().unwrap();
        let repo_path = dir.path();

        let mut repo = Repository::init(repo_path).unwrap();
        let (author, signing_key) = arc_core::store::author::test_keypair();
        repo.set_identity(author, signing_key);

        std::fs::write(repo_path.join("tracked.rs"), "fn a() {}\n").unwrap();
        let _ = repo.snap("initial tracked file", false).unwrap();

        std::fs::write(repo_path.join("tracked.rs"), "fn a() { let x = 1; }\n").unwrap();
        std::fs::write(repo_path.join("new.rs"), "fn b() {}\n").unwrap();

        let req = RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: 8,
            method: "get_file_states".to_string(),
            params: Some(json!({ "path": repo_path.display().to_string() })),
        };

        let resp = handle_request(req).await;
        let value = serde_json::to_value(&resp).unwrap();

        assert_eq!(value["jsonrpc"], "2.0");
        assert_eq!(value["id"], 8);
        assert!(value["error"].is_null(), "unexpected error: {value}");

        let result = value["result"]
            .as_array()
            .expect("result should be an array");
        assert!(!result.is_empty(), "expected at least one file state");
        for item in result {
            assert!(item["file_path"].is_string());
            assert!(item["status"].is_string());
            let status = item["status"].as_str().unwrap();
            assert!(
                matches!(
                    status,
                    "modified" | "untracked" | "conflict" | "ai_generated"
                ),
                "unexpected status value: {status}"
            );
        }
    }
}
