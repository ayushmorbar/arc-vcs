use std::path::PathBuf;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};

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

async fn get_object(
    State(state): State<AppState>,
    Path(hash): Path<String>,
) -> impl IntoResponse {
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

/// Start a dumb HTTP server exposing the arc CAS of the current directory.
///
/// Two endpoints:
/// - `GET /views/:name`   → [`arc_core::store::view::View`] serialised as JSON
/// - `GET /objects/:hash` → raw `bincode` bytes of a [`arc_core::store::change::Change`]
///
/// The server is intentionally read-only. All `Change` objects carry an
/// Ed25519 signature, so a client that runs `Repository::verify_graph`
/// after fetching is guaranteed to detect any in-transit tampering.
pub async fn serve(port: u16) -> anyhow::Result<()> {
    let repo_root = std::env::current_dir()?;
    let state = AppState { repo_root };
    let app = Router::new()
        .route("/views/{name}", get(get_view))
        .route("/objects/{hash}", get(get_object))
        .with_state(state);
    let addr = format!("0.0.0.0:{port}");
    println!("arc server listening on {addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
