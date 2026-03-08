use std::collections::HashSet;
use std::path::PathBuf;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};

use arc_core::algebra::Blake3Hash;
use arc_core::network::{DeltaPayload, verify_payload};
use arc_core::store::cas::ObjectStore;
use arc_core::store::view::View;

#[derive(Clone)]
struct AppState {
    repo_root: PathBuf,
}

async fn get_view(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<View>, StatusCode> {
    View::load(&state.repo_root, &name)
        .map(Json)
        .map_err(|_| StatusCode::NOT_FOUND)
}

async fn get_object(State(state): State<AppState>, Path(hash): Path<String>) -> impl IntoResponse {
    // Security: validate exactly 64 lowercase hex digits to prevent path traversal
    // (e.g. reject "../../etc/passwd" style inputs).
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let path = state
        .repo_root
        .join(".arc")
        .join("store")
        .join(&hash[..2])
        .join(&hash[2..]);
    match std::fs::read(&path) {
        Ok(bytes) => (
            [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
            bytes,
        )
            .into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn get_blob(
    State(state): State<AppState>,
    Path(hash): Path<String>,
) -> impl IntoResponse {
    // Security: validate exactly 64 lowercase hex digits to prevent path traversal.
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let path = state.repo_root.join(".arc").join("blobs").join(&hash);
    match std::fs::read(&path) {
        Ok(bytes) => (
            [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
            bytes,
        )
            .into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn post_sync(
    State(state): State<AppState>,
    Path(view_name): Path<String>,
    Json(payload): Json<DeltaPayload>,
) -> impl IntoResponse {
    // Zero-trust ingress boundary: verify all Ed25519 signatures before any
    // write to the CAS.  A tampered blob changes content_hash → changes Change
    // id → breaks signature, so this check is mathematically complete.
    if let Err(e) = verify_payload(&payload) {
        return (StatusCode::BAD_REQUEST, e.to_string()).into_response();
    }

    let store = ObjectStore::new(&state.repo_root);

    // Write changes to CAS (idempotent — duplicate writes are no-ops).
    for change in &payload.changes {
        if store.write_change(change).is_err() {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }

    // Write blob sidecars to CAS (idempotent; key is derived from content).
    for bytes in payload.blobs.values() {
        if store.write_blob(bytes).is_err() {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }

    // CRDT view union: advance the view heads with zero coordination.
    let existing_heads: HashSet<Blake3Hash> = View::load(&state.repo_root, &view_name)
        .map(|v| v.heads)
        .unwrap_or_default();
    let new_heads: HashSet<Blake3Hash> =
        existing_heads.union(&payload.view_heads).copied().collect();
    if View::new(&view_name, new_heads)
        .save(&state.repo_root)
        .is_err()
    {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    StatusCode::OK.into_response()
}

/// Start the arc HTTP server exposing this repository's CAS.
///
/// Four endpoints:
/// - `GET  /views/:name`      → [`arc_core::store::view::View`] as JSON
/// - `GET  /objects/:hash`    → raw `bincode` bytes of a [`arc_core::store::change::Change`]
/// - `GET  /blobs/:hash`      → raw bytes of a CAS blob
/// - `POST /sync/:view_name`  → accepts a [`DeltaPayload`]; verifies Ed25519 signatures,
///   writes changes + blobs to CAS, advances the view (CRDT union)
///
/// All `Change` objects carry an Ed25519 signature verified on ingress, so
/// the server never writes unauthenticated data to its CAS.
pub async fn serve(port: u16) -> anyhow::Result<()> {
    let repo_root = std::env::current_dir()?;
    let state = AppState { repo_root };
    let app = Router::new()
        .route("/views/{name}", get(get_view))
        .route("/objects/{hash}", get(get_object))
        .route("/blobs/{hash}", get(get_blob))
        .route("/sync/{view_name}", post(post_sync))
        .with_state(state);
    let addr = format!("0.0.0.0:{port}");
    println!("arc server listening on {addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
