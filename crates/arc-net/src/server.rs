use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use http_body_util::BodyExt;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use arc_algebra_types::{Atom, Blake3Hash};
use arc_change::Change;
use arc_network::{DeltaPayload, SyncResponse, verify_payload};
use arc_store_cas::ObjectStore;
use arc_store_types::author::Author;
use arc_store_view::View;

fn write_change(store: &ObjectStore, change: &Change) -> Result<(), ()> {
    let bytes = bincode::serialize(change).map_err(|_| ())?;
    store.write_object(&change.id, &bytes).map(|_| ()).map_err(|_| ())
}

#[derive(Clone)]
struct AppState {
    repo_root: PathBuf,
    server_author: Author,
    /// Raw Ed25519 seed bytes — `SigningKey::from_bytes` is called per-request
    /// so the state never holds a non-Clone type.
    server_signing_seed: [u8; 32],
}

// ── Server identity ────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
struct ServerIdentity {
    canonical_id: String,
    secret_key_bytes: [u8; 32],
}

fn load_or_generate_server_identity(
    repo_root: &std::path::Path,
) -> anyhow::Result<(Author, [u8; 32])> {
    let identity_path = repo_root.join(".arc").join("server_identity.json");
    if identity_path.exists() {
        let json = std::fs::read_to_string(&identity_path)?;
        let id: ServerIdentity = serde_json::from_str(&json)?;
        let author = arc_store_types::author::server_author_from_seed(
            &id.canonical_id,
            &id.secret_key_bytes,
        );
        Ok((author, id.secret_key_bytes))
    } else {
        let (author, seed) = arc_store_types::author::generate_server_keypair_seed("arc-server");
        let id = ServerIdentity {
            canonical_id: match &author {
                Author::Server { canonical_id, .. } => canonical_id.clone(),
                _ => unreachable!(),
            },
            secret_key_bytes: seed,
        };
        if let Some(parent) = identity_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&identity_path, serde_json::to_string_pretty(&id)?)?;
        Ok((author, seed))
    }
}

/// Returns `true` when the author represents a transient or ephemeral identity
/// that should be collapsed into a canonical server identity on ingress.
///
/// Phase 40: the trigger is a strict enum match on `Author::Transient`
/// rather than brittle string heuristics.  CI/CD runners and AI agents that
/// set `ARC_EPHEMERAL_RUNNER` automatically push with a `Transient` author,
/// which flows through here and triggers the Phase 39 Identity Collapse.
fn is_transient_author(author: &Author) -> bool {
    matches!(author, Author::Transient { .. })
}

async fn get_view(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<View>, StatusCode> {
    View::load(&state.repo_root, &name).map(Json).map_err(|_| StatusCode::NOT_FOUND)
}

async fn get_object(State(state): State<AppState>, Path(hash): Path<String>) -> impl IntoResponse {
    // Security: validate exactly 64 lowercase hex digits to prevent path traversal
    // (e.g. reject "../../etc/passwd" style inputs).
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let path = state.repo_root.join(".arc").join("store").join(&hash[..2]).join(&hash[2..]);
    match std::fs::read(&path) {
        Ok(bytes) => ([(axum::http::header::CONTENT_TYPE, "application/octet-stream")], bytes)
            .into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn get_blob(State(state): State<AppState>, Path(hash): Path<String>) -> impl IntoResponse {
    // Security: validate exactly 64 lowercase hex digits to prevent path traversal.
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let path = state.repo_root.join(".arc").join("blobs").join(&hash);
    match std::fs::read(&path) {
        Ok(bytes) => ([(axum::http::header::CONTENT_TYPE, "application/octet-stream")], bytes)
            .into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

/// `PUT /blobs/:hash` — streaming blob intake with BLAKE3 hash verification.
///
/// The client streams raw blob bytes.  The server writes them to a temp file
/// while simultaneously computing the BLAKE3 hash.  After the stream ends,
/// the computed hash is compared to the path parameter: match → atomic rename
/// into `.arc/blobs/`; mismatch → temp file deleted → 400.
///
/// Idempotent: a second PUT for a blob that already exists returns 200.
async fn put_blob(
    State(state): State<AppState>,
    Path(hash): Path<String>,
    request: axum::extract::Request,
) -> impl IntoResponse {
    // Security: validate exactly 64 lowercase hex digits (path traversal guard).
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let blob_path = state.repo_root.join(".arc").join("blobs").join(&hash);
    // Idempotency: if the blob already exists, nothing to do.
    if blob_path.exists() {
        return StatusCode::OK.into_response();
    }

    let tmp_dir = state.repo_root.join(".arc").join("tmp");
    if let Err(e) = tokio::fs::create_dir_all(&tmp_dir).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    let tmp_path = tmp_dir.join(format!("{hash}.tmp"));

    let mut tmp_file = match tokio::fs::File::create(&tmp_path).await {
        Ok(f) => f,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    // Stream request body frame-by-frame: feed each chunk to the BLAKE3
    // hasher AND write it directly to the temp file — no RAM buffering.
    let mut hasher = blake3::Hasher::new();
    let mut body = request.into_body();

    loop {
        match body.frame().await {
            Some(Ok(frame)) => {
                if let Ok(chunk) = frame.into_data() {
                    hasher.update(&chunk);
                    if let Err(e) = tmp_file.write_all(&chunk).await {
                        let _ = tokio::fs::remove_file(&tmp_path).await;
                        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
                    }
                }
            }
            Some(Err(e)) => {
                let _ = tokio::fs::remove_file(&tmp_path).await;
                return (StatusCode::BAD_REQUEST, e.to_string()).into_response();
            }
            None => break,
        }
    }

    if let Err(e) = tmp_file.flush().await {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    drop(tmp_file);

    // Verify the BLAKE3 hash matches the path parameter.
    let computed_hex: String =
        hasher.finalize().as_bytes().iter().map(|b| format!("{b:02x}")).collect();
    if computed_hex != hash {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return (
            StatusCode::BAD_REQUEST,
            "hash mismatch: computed hash does not match path parameter",
        )
            .into_response();
    }

    // Atomic rename into the blobs directory.
    if let Some(parent) = blob_path.parent()
        && let Err(e) = tokio::fs::create_dir_all(parent).await
    {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    if let Err(e) = tokio::fs::rename(&tmp_path, &blob_path).await {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }

    StatusCode::CREATED.into_response()
}

async fn post_sync(
    State(state): State<AppState>,
    Path(view_name): Path<String>,
    Json(payload): Json<DeltaPayload>,
) -> impl IntoResponse {
    // Stage 1: Zero-trust signature verification before any CAS write.
    if let Err(e) = verify_payload(&payload) {
        return (StatusCode::BAD_REQUEST, e.to_string()).into_response();
    }

    let store = ObjectStore::new(&state.repo_root);

    // Stage 2: Blob pre-existence check.
    // All blobs referenced by atoms must already be in CAS (uploaded via
    // PUT /blobs/:hash).  If any are missing, return 409 with the list so
    // the client can re-upload them before retrying.
    let mut missing_blobs: Vec<String> = Vec::new();
    for change in &payload.changes {
        for atom in &change.atoms {
            let hash = match atom {
                Atom::Insert { content_hash, .. } => content_hash,
                Atom::Delete { prior_hash, .. } => prior_hash,
                _ => continue,
            };
            if !store.contains_blob(hash) {
                let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
                if !missing_blobs.contains(&hex) {
                    missing_blobs.push(hex);
                }
            }
        }
    }
    if !missing_blobs.is_empty() {
        return (StatusCode::CONFLICT, Json(missing_blobs)).into_response();
    }

    // Stage 3: Topological sort (Kahn's algorithm) + Dual-Provenance Identity
    // Collapsing with Cryptographic Cascade.
    //
    // Cascade rule: the rewrite trigger for a Change C is:
    //   is_transient(C.author)  ||  any dep of C was rewritten
    //
    // Because deps are included in compute_id, remapping a dep changes C's
    // hash, which invalidates C's original signature.  Re-signing under
    // Author::Server is therefore required in both cases.

    let payload_ids: HashSet<Blake3Hash> = payload.changes.iter().map(|c| c.id).collect();

    // Build in-degree map (counting only intra-payload edges).
    let mut in_degree: HashMap<Blake3Hash, usize> =
        payload.changes.iter().map(|c| (c.id, 0usize)).collect();

    // Build reverse adjacency: parent_id → Vec<child_id> (child depends on parent).
    let mut dependants: HashMap<Blake3Hash, Vec<Blake3Hash>> = HashMap::new();
    for change in &payload.changes {
        for &dep in &change.deps {
            if payload_ids.contains(&dep) {
                *in_degree.get_mut(&change.id).unwrap() += 1;
                dependants.entry(dep).or_default().push(change.id);
            }
        }
    }

    // Kahn queue: start with all zero-in-degree nodes.
    let mut queue: VecDeque<Blake3Hash> =
        in_degree.iter().filter(|(_, deg)| **deg == 0).map(|(id, _)| *id).collect();

    let change_map: HashMap<Blake3Hash, &Change> =
        payload.changes.iter().map(|c| (c.id, c)).collect();

    let Author::Server { .. } = &state.server_author else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };

    // Internal map: original Blake3Hash → canonical Blake3Hash.
    let mut rewritten_map: HashMap<Blake3Hash, Blake3Hash> = HashMap::new();
    let mut processed = 0usize;

    while let Some(id) = queue.pop_front() {
        let change = change_map[&id];
        processed += 1;

        // Remap deps through the rewritten_map accumulated so far.
        let remapped_deps: HashSet<Blake3Hash> = change
            .deps
            .iter()
            .map(|&dep| rewritten_map.get(&dep).copied().unwrap_or(dep))
            .collect();

        let deps_were_remapped = remapped_deps != change.deps;
        // Cascade rule: collapse if transient author OR any dep was rewritten.
        let trigger_collapse = is_transient_author(&change.author) || deps_were_remapped;

        if trigger_collapse {
            let canonical = Change::new_canonical_from_seed(
                remapped_deps,
                change.atoms.clone(),
                change.intent.clone(),
                state.server_author.clone(),
                &state.server_signing_seed,
                change.id,
            );
            let canonical_id = canonical.id;

            // Write BOTH: original stays as the SLSA L4 audit root; canonical
            // is the version distributed to other clients.
            if write_change(&store, change).is_err() || write_change(&store, &canonical).is_err() {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            rewritten_map.insert(change.id, canonical_id);
        } else {
            if write_change(&store, change).is_err() {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        }

        // Decrement in-degree for intra-payload dependants.
        for &child_id in dependants.get(&id).into_iter().flatten() {
            let deg = in_degree.get_mut(&child_id).unwrap();
            *deg -= 1;
            if *deg == 0 {
                queue.push_back(child_id);
            }
        }
    }

    // Cycle detection: if not all nodes were processed, there's a dep cycle.
    if processed < payload.changes.len() {
        return (StatusCode::BAD_REQUEST, "cyclic dependency in payload").into_response();
    }

    // Stage 4: CRDT view union with remapped heads.
    let existing_heads: HashSet<Blake3Hash> =
        View::load(&state.repo_root, &view_name).map(|v| v.heads).unwrap_or_default();
    // Map transient payload heads to their canonical equivalents before union.
    let canonical_payload_heads: HashSet<Blake3Hash> =
        payload.view_heads.iter().map(|&h| rewritten_map.get(&h).copied().unwrap_or(h)).collect();
    let new_heads: HashSet<Blake3Hash> =
        existing_heads.union(&canonical_payload_heads).copied().collect();

    if View::new(&view_name, new_heads.clone()).save(&state.repo_root).is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    // Stage 5: Return SyncResponse with canonical view heads and rewrite map.
    let rewritten_map_str: HashMap<String, String> = rewritten_map
        .iter()
        .map(|(old, new)| {
            let old_hex = old.iter().map(|b| format!("{b:02x}")).collect();
            let new_hex = new.iter().map(|b| format!("{b:02x}")).collect();
            (old_hex, new_hex)
        })
        .collect();

    Json(SyncResponse { view_heads: new_heads, rewritten_map: rewritten_map_str }).into_response()
}

/// Start the arc HTTP server exposing this repository's CAS.
///
/// Five endpoints:
/// - `GET  /views/:name`      → [`arc_store_view::View`] as JSON
/// - `GET  /objects/:hash`    → raw `bincode` bytes of a [`arc_change::Change`]
/// - `GET  /blobs/:hash`      → raw bytes of a CAS blob
/// - `PUT  /blobs/:hash`      → streaming blob intake with BLAKE3 verification
/// - `POST /sync/:view_name`  → accepts a [`DeltaPayload`]; verifies Ed25519 signatures;
///   writes changes to CAS; runs Dual-Provenance Identity Collapsing; advances view
///
/// Server signing identity is loaded from (or generated at) `.arc/server_identity.json`.
/// All `Change` objects carry an Ed25519 signature verified on ingress, so
/// the server never writes unauthenticated data to its CAS.
pub async fn serve(port: u16) -> anyhow::Result<()> {
    let repo_root = std::env::current_dir()?;
    let (server_author, server_signing_seed) = load_or_generate_server_identity(&repo_root)?;
    let state = AppState { repo_root, server_author, server_signing_seed };
    let app = Router::new()
        .route("/views/{name}", get(get_view))
        .route("/objects/{hash}", get(get_object))
        .route("/blobs/{hash}", get(get_blob).put(put_blob))
        .route("/sync/{view_name}", post(post_sync))
        .with_state(state);
    let addr = format!("0.0.0.0:{port}");
    println!("arc server listening on {addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
