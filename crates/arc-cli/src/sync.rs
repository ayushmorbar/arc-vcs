use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::repo::Repository;
use crate::store_compat::ObjectStoreChangeExt;
use arc_algebra_types::Atom;
use arc_algebra_types::Blake3Hash;
use arc_change::Change;
use arc_keyring::{ArcIdentity, IdentityManager, KeyringSessionFacade};
use arc_network::{DeltaPayload, NetworkClient, SyncResponse};
use arc_store_view::View;

/// Resolve `name_or_path` to a concrete URL or filesystem path.
///
/// A value is treated as a *direct* reference when it starts with
/// `http://`, `https://`, `.`, `/`, or contains a path separator (`/` or
/// `\`).  Otherwise it is looked up as a named remote alias in the
/// repository's `.arc/config.json`.
fn resolve_remote(local: &Repository, name_or_path: &str) -> anyhow::Result<String> {
    let is_direct = name_or_path.starts_with("http://")
        || name_or_path.starts_with("https://")
        || name_or_path.starts_with('.')
        || name_or_path.starts_with('/')
        || name_or_path.contains('\\')
        || name_or_path.contains('/');
    if is_direct {
        return Ok(name_or_path.to_string());
    }
    let config = local.read_config()?;
    config.remotes.get(name_or_path).cloned().ok_or_else(|| {
        anyhow::anyhow!(
            "no remote named '{}'. Add one with: arc remote add {} <url>",
            name_or_path,
            name_or_path
        )
    })
}

/// Fetch missing changes from a remote repository's view into the local
/// repository.
///
/// `remote_path` is either a local filesystem path, an `http://` /
/// `https://` URL pointing at an `arc serve` instance, **or** a named
/// remote alias registered with `arc remote add`.
///
/// The Bounded BFS algorithm is identical in both cases: any change already
/// present in the local store is a causal cut-point — its ancestors are
/// guaranteed to be present locally, so they are not enqueued.
///
/// Returns the remote view's heads.
pub fn fetch(
    local: &mut Repository,
    remote_path: &str,
    view_name: &str,
) -> anyhow::Result<HashSet<Blake3Hash>> {
    let resolved = resolve_remote(local, remote_path)?;
    if resolved.starts_with("http://") || resolved.starts_with("https://") {
        return fetch_http(local, &resolved, view_name);
    }
    fetch_local(local, &resolved, view_name)
}

fn fetch_local(
    local: &mut Repository,
    remote_path: &str,
    view_name: &str,
) -> anyhow::Result<HashSet<Blake3Hash>> {
    let remote = Repository::open(remote_path)?;
    let remote_view = View::load(&remote.shared_root, view_name)
        .map_err(|e| anyhow::anyhow!("failed to load remote view '{view_name}': {e}"))?;

    let mut queue: VecDeque<Blake3Hash> = remote_view.heads.iter().copied().collect();
    let mut visited = HashSet::new();

    while let Some(id) = queue.pop_front() {
        if !visited.insert(id) {
            continue;
        }

        // Bounded BFS: if the local store already has this change,
        // all its ancestors are causally guaranteed to be present.
        if local.store.read_change(&id).is_ok() {
            if local.graph.load().get(&id).is_none() {
                let change = local.store.read_change(&id).unwrap();
                local.graph_add_change(change);
            }
            continue;
        }
        let change = remote
            .store
            .read_change(&id)
            .map_err(|e| anyhow::anyhow!("failed to read change from remote CAS: {e}"))?;
        local
            .store
            .write_change(&change)
            .map_err(|e| anyhow::anyhow!("failed to write change to local CAS: {e}"))?;

        // Copy referenced content blobs so apply_change can materialise state.
        for atom in &change.atoms {
            match atom {
                Atom::Insert { content_hash, .. } => {
                    if !local.store.contains_blob(content_hash)
                        && let Ok(bytes) = remote.store.read_blob(content_hash)
                    {
                        let _ = local.store.write_blob(&bytes);
                    }
                }
                Atom::Delete { prior_hash, .. } => {
                    if !local.store.contains_blob(prior_hash)
                        && let Ok(bytes) = remote.store.read_blob(prior_hash)
                    {
                        let _ = local.store.write_blob(&bytes);
                    }
                }
                _ => {}
            }
        }

        for &dep in &change.deps {
            if !visited.contains(&dep) {
                queue.push_back(dep);
            }
        }
        local.graph_add_change(change);
    }

    Ok(remote_view.heads)
}

/// Bounded BFS fetch from a remote HTTP server.
///
/// Traverses the DAG backward from `roots`, downloading every [`Change`]
/// (via `GET /objects/{hex}`) and its blob sidecars (via `GET /blobs/{hex}`)
/// that the local CAS is missing.  Returns the number of objects fetched.
///
/// This is the shared primitive used by both [`fetch_http`] (initial clone /
/// pull) and the synchronization-closure step in [`push_http`] (fetching
/// canonical objects written by the server during Identity Collapsing).
fn bfs_fetch_changes(
    client: &reqwest::blocking::Client,
    remote_url: &str,
    local: &mut Repository,
    roots: &HashSet<Blake3Hash>,
    pb: &indicatif::ProgressBar,
) -> anyhow::Result<usize> {
    let mut queue: VecDeque<Blake3Hash> = roots.iter().copied().collect();
    let mut visited: HashSet<Blake3Hash> = HashSet::new();

    while let Some(id) = queue.pop_front() {
        if !visited.insert(id) {
            continue;
        }

        if local.store.read_change(&id).is_ok() {
            if local.graph.load().get(&id).is_none() {
                let change = local.store.read_change(&id).unwrap();
                local.graph_add_change(change);
            }
            continue;
        }

        let hex: String = id.iter().map(|b| format!("{b:02x}")).collect();
        pb.set_message(format!("Downloading {}...", &hex[..8]));
        let obj_url = format!("{remote_url}/objects/{hex}");
        let bytes = client
            .get(&obj_url)
            .send()
            .map_err(|e| anyhow::anyhow!("HTTP GET {obj_url} failed: {e}"))?
            .error_for_status()
            .map_err(|e| anyhow::anyhow!("server returned error for {obj_url}: {e}"))?
            .bytes()
            .map_err(|e| anyhow::anyhow!("failed to read object bytes: {e}"))?;
        let change: Change = bincode::deserialize(&bytes)
            .map_err(|e| anyhow::anyhow!("failed to deserialise change {hex}: {e}"))?;
        local
            .store
            .write_change(&change)
            .map_err(|e| anyhow::anyhow!("failed to write change {hex} to local CAS: {e}"))?;
        for &dep in &change.deps {
            if !visited.contains(&dep) {
                queue.push_back(dep);
            }
        }
        for atom in &change.atoms {
            match atom {
                Atom::Insert { content_hash, .. } if !local.store.contains_blob(content_hash) => {
                    let blob_hex: String =
                        content_hash.iter().map(|b| format!("{b:02x}")).collect();
                    let blob_url = format!("{remote_url}/blobs/{blob_hex}");
                    let blob_bytes = client
                        .get(&blob_url)
                        .send()
                        .map_err(|e| anyhow::anyhow!("HTTP GET {blob_url} failed: {e}"))?
                        .error_for_status()
                        .map_err(|_| {
                            anyhow::anyhow!(
                                "Corrupt remote: missing blob {blob_hex} (change {hex})"
                            )
                        })?
                        .bytes()
                        .map_err(|e| anyhow::anyhow!("failed to read blob bytes: {e}"))?;
                    local
                        .store
                        .write_blob(&blob_bytes)
                        .map_err(|e| anyhow::anyhow!("failed to write blob {blob_hex}: {e}"))?;
                }
                Atom::Delete { prior_hash, .. } if !local.store.contains_blob(prior_hash) => {
                    let blob_hex: String = prior_hash.iter().map(|b| format!("{b:02x}")).collect();
                    let blob_url = format!("{remote_url}/blobs/{blob_hex}");
                    let blob_bytes = client
                        .get(&blob_url)
                        .send()
                        .map_err(|e| anyhow::anyhow!("HTTP GET {blob_url} failed: {e}"))?
                        .error_for_status()
                        .map_err(|_| {
                            anyhow::anyhow!(
                                "Corrupt remote: missing blob {blob_hex} (change {hex})"
                            )
                        })?
                        .bytes()
                        .map_err(|e| anyhow::anyhow!("failed to read blob bytes: {e}"))?;
                    local
                        .store
                        .write_blob(&blob_bytes)
                        .map_err(|e| anyhow::anyhow!("failed to write blob {blob_hex}: {e}"))?;
                }
                _ => {}
            }
        }
        local.graph_add_change(change);
    }
    Ok(visited.len())
}

fn fetch_http(
    local: &mut Repository,
    remote_url: &str,
    view_name: &str,
) -> anyhow::Result<HashSet<Blake3Hash>> {
    let url = format!("{remote_url}/views/{view_name}");
    let remote_view: View = reqwest::blocking::get(&url)
        .map_err(|e| anyhow::anyhow!("HTTP GET {url} failed: {e}"))?
        .error_for_status()
        .map_err(|e| anyhow::anyhow!("server returned error for {url}: {e}"))?
        .json::<View>()
        .map_err(|e| anyhow::anyhow!("failed to deserialise remote view: {e}"))?;

    let client = reqwest::blocking::Client::new();
    let pb = indicatif::ProgressBar::new_spinner();
    pb.enable_steady_tick(Duration::from_millis(80));
    pb.set_message(format!("Fetching view '{view_name}' from {remote_url}..."));

    let fetched = bfs_fetch_changes(&client, remote_url, local, &remote_view.heads, &pb)?;
    pb.finish_with_message(format!("Fetched {fetched} objects from {remote_url}."));

    Ok(remote_view.heads)
}

/// Pull changes from a remote repository's view and merge them into the
/// local active view.
///
/// This is `fetch` followed by `merge_heads` — the CRDT sync primitive.
pub fn pull(local: &mut Repository, remote_path: &str, view_name: &str) -> anyhow::Result<()> {
    let resolved = resolve_remote(local, remote_path)?;
    let remote_heads = if resolved.starts_with("http://") || resolved.starts_with("https://") {
        match pull_http_signed(local, &resolved, view_name) {
            Ok(heads) => heads,
            Err(error) => {
                if allow_unsigned_sync_fallback() {
                    eprintln!(
                        "warning: signed pull unavailable ({error}); falling back to compatibility fetch"
                    );
                    fetch_http(local, &resolved, view_name)?
                } else {
                    return Err(anyhow::anyhow!(
                        "signed pull failed: {error}. Set ARC_ALLOW_UNSIGNED_SYNC_FALLBACK=1 to permit compatibility fallback"
                    ));
                }
            }
        }
    } else {
        fetch(local, remote_path, view_name)?
    };
    local.merge_heads(&remote_heads)?;
    Ok(())
}

/// Push local changes from a view to a remote repository.
///
/// `remote_path` is either a local filesystem path, an `http://` /
/// `https://` URL, or a named remote alias registered with `arc remote add`.
///
/// The algorithm is the CRDT dual of [`fetch`]: compute the delta of Changes
/// the remote is missing, bundle their blob sidecars, and deliver atomically.
pub fn push(local: &mut Repository, remote_path: &str, view_name: &str) -> anyhow::Result<()> {
    let resolved = resolve_remote(local, remote_path)?;
    if resolved.starts_with("http://") || resolved.starts_with("https://") {
        return push_http(local, &resolved, view_name);
    }
    push_local(local, &resolved, view_name)
}

fn push_local(local: &Repository, remote_path: &str, view_name: &str) -> anyhow::Result<()> {
    let remote = Repository::open(remote_path)?;
    let local_view = View::load(&local.shared_root, view_name)
        .map_err(|e| anyhow::anyhow!("failed to load local view '{view_name}': {e}"))?;
    let remote_heads: HashSet<Blake3Hash> =
        View::load(&remote.shared_root, view_name).map(|v| v.heads).unwrap_or_default();

    // BFS from local heads; cut when we reach a change already in remote CAS.
    let mut queue: VecDeque<Blake3Hash> = local_view.heads.iter().copied().collect();
    let mut visited: HashSet<Blake3Hash> = HashSet::new();
    let mut delta: Vec<Change> = Vec::new();

    while let Some(id) = queue.pop_front() {
        if !visited.insert(id) {
            continue;
        }
        // Remote already has this change and all its ancestors (CAS invariant).
        if remote.store.read_change(&id).is_ok() {
            continue;
        }
        let change = local
            .store
            .read_change(&id)
            .map_err(|e| anyhow::anyhow!("failed to read local change: {e}"))?;
        for &dep in &change.deps {
            if !visited.contains(&dep) {
                queue.push_back(dep);
            }
        }
        delta.push(change);
    }

    // Write delta changes + blob sidecars to remote CAS (idempotent).
    for change in &delta {
        remote
            .store
            .write_change(change)
            .map_err(|e| anyhow::anyhow!("failed to write change to remote: {e}"))?;
        for atom in &change.atoms {
            match atom {
                Atom::Insert { content_hash, .. } if !remote.store.contains_blob(content_hash) => {
                    let bytes = local
                        .store
                        .read_blob(content_hash)
                        .map_err(|e| anyhow::anyhow!("missing local blob: {e}"))?;
                    remote
                        .store
                        .write_blob(&bytes)
                        .map_err(|e| anyhow::anyhow!("failed to write blob to remote: {e}"))?;
                }
                Atom::Delete { prior_hash, .. } if !remote.store.contains_blob(prior_hash) => {
                    let bytes = local
                        .store
                        .read_blob(prior_hash)
                        .map_err(|e| anyhow::anyhow!("missing local blob: {e}"))?;
                    remote
                        .store
                        .write_blob(&bytes)
                        .map_err(|e| anyhow::anyhow!("failed to write blob to remote: {e}"))?;
                }
                _ => {}
            }
        }
    }

    // CRDT view union — atomic rename prevents corruption under concurrent access.
    let new_heads: HashSet<Blake3Hash> = remote_heads.union(&local_view.heads).copied().collect();
    View::new(view_name, new_heads)
        .save(&remote.shared_root)
        .map_err(|e| anyhow::anyhow!("failed to save remote view: {e}"))?;

    println!("Pushed {} change(s) to {} [view: {}].", delta.len(), remote_path, view_name);
    Ok(())
}

fn push_http(local: &mut Repository, remote_url: &str, view_name: &str) -> anyhow::Result<()> {
    let client = reqwest::blocking::Client::builder()
        .user_agent(concat!("arc-vcs/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| anyhow::anyhow!("failed to build HTTP client: {e}"))?;

    // Fetch remote view heads (empty set if view doesn't exist yet on server).
    let remote_heads: HashSet<Blake3Hash> = {
        let url = format!("{remote_url}/views/{view_name}");
        match client.get(&url).send() {
            Ok(resp) if resp.status().is_success() => {
                resp.json::<View>().map(|v| v.heads).unwrap_or_default()
            }
            _ => HashSet::new(),
        }
    };

    let local_view = View::load(&local.shared_root, view_name)
        .map_err(|e| anyhow::anyhow!("failed to load local view '{view_name}': {e}"))?;

    // BFS delta: collect local changes the remote is missing.
    let mut queue: VecDeque<Blake3Hash> = local_view.heads.iter().copied().collect();
    let mut visited: HashSet<Blake3Hash> = HashSet::new();
    let mut delta: Vec<Change> = Vec::new();

    while let Some(id) = queue.pop_front() {
        if !visited.insert(id) {
            continue;
        }
        if remote_heads.contains(&id) {
            continue;
        }
        let change = local
            .store
            .read_change(&id)
            .map_err(|e| anyhow::anyhow!("failed to read local change: {e}"))?;
        for &dep in &change.deps {
            if !visited.contains(&dep) {
                queue.push_back(dep);
            }
        }
        delta.push(change);
    }

    // Collect unique blob hashes referenced by all delta atoms.
    let mut blob_hashes: Vec<Blake3Hash> = Vec::new();
    let mut seen_blobs: HashSet<Blake3Hash> = HashSet::new();
    for change in &delta {
        for atom in &change.atoms {
            match atom {
                Atom::Insert { content_hash, .. } => {
                    if seen_blobs.insert(*content_hash) {
                        blob_hashes.push(*content_hash);
                    }
                }
                Atom::Delete { prior_hash, .. } => {
                    if seen_blobs.insert(*prior_hash) {
                        blob_hashes.push(*prior_hash);
                    }
                }
                _ => {}
            }
        }
    }

    let n_changes = delta.len();
    let n_blobs = blob_hashes.len();

    // Phase 1: Upload blobs out-of-band via PUT /blobs/:hash (streaming from
    // disk — no RAM buffering regardless of blob size).
    // Pre-scan total bytes upfront so the hierarchical progress bar can show
    // bytes-uploaded/total and per-file bars for blobs above the threshold.
    if n_blobs > 0 {
        let total_blob_bytes: u64 = blob_hashes
            .iter()
            .map(|h| local.store.blob_file_path(h).metadata().map(|m| m.len()).unwrap_or(0))
            .sum();
        upload_blobs(&client, remote_url, local, &blob_hashes, total_blob_bytes)?;
    }

    // Phase 2: POST DeltaPayload (metadata only, no inline blobs).
    let payload = DeltaPayload { changes: delta, view_heads: local_view.heads.clone() };
    let pb = indicatif::ProgressBar::new_spinner();
    pb.enable_steady_tick(Duration::from_millis(80));
    pb.set_message(format!("Pushing {n_changes} change(s) to {remote_url}..."));
    let signed_sync = (|| -> anyhow::Result<SyncResponse> {
        let identity = load_active_identity_for_sync()?;
        let runtime = tokio::runtime::Runtime::new()
            .map_err(|e| anyhow::anyhow!("failed to create tokio runtime: {e}"))?;
        let network = NetworkClient::new()
            .map_err(|e| anyhow::anyhow!("failed to create signed network client: {e}"))?;
        let local_changes: std::collections::HashMap<Blake3Hash, Change> =
            payload.changes.iter().cloned().map(|c| (c.id, c)).collect();
        runtime
            .block_on(network.push_changes(
                remote_url,
                view_name,
                &payload.view_heads,
                &remote_heads,
                &local_changes,
                &identity,
            ))
            .map_err(|e| anyhow::anyhow!("signed push failed: {e}"))
    })();

    let sync_resp = match signed_sync {
        Ok(response) => response,
        Err(error) => {
            if allow_unsigned_sync_fallback() {
                eprintln!(
                    "warning: signed push unavailable ({error}); falling back to compatibility transport"
                );
                post_payload_with_retry(&client, remote_url, view_name, &payload, local, &pb)?
            } else {
                return Err(anyhow::anyhow!(
                    "signed push failed: {error}. Set ARC_ALLOW_UNSIGNED_SYNC_FALLBACK=1 to permit compatibility fallback"
                ));
            }
        }
    };

    // Phase 3: Process identity collapsing result.
    // If the server collapsed any transient-author Changes under its canonical
    // identity, fetch the new canonical objects into the local CAS and advance
    // the local view pointer, then run a conservative GC to prune the now-
    // orphaned transient-author metadata (conservative = respects OpLog so
    // `arc undo` remains safe; CAS dedup means old metadata is negligible bytes).
    if !sync_resp.rewritten_map.is_empty() {
        pb.set_message("Fetching canonical objects after identity collapse...");
        bfs_fetch_changes(&client, remote_url, local, &sync_resp.view_heads, &pb)?;
        View::new(view_name, sync_resp.view_heads).save(&local.shared_root).map_err(|e| {
            anyhow::anyhow!("failed to update local view after identity collapse: {e}")
        })?;
        if let Err(e) = local.gc() {
            // Non-fatal: repository is fully consistent; old metadata lingers
            // in the CAS until the next OpLog compaction prunes the references.
            eprintln!("warning: post-collapse GC failed: {e}");
        }
    }

    pb.finish_with_message(format!(
        "Pushed {n_changes} change(s) to {remote_url} [view: {view_name}]."
    ));
    Ok(())
}

fn pull_http_signed(
    local: &mut Repository,
    remote_url: &str,
    view_name: &str,
) -> anyhow::Result<HashSet<Blake3Hash>> {
    let identity = load_active_identity_for_sync()?;
    let remote_author_public_key = load_expected_remote_author_key()?;

    let local_frontier =
        View::load(&local.shared_root, view_name).map(|view| view.heads).unwrap_or_default();

    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| anyhow::anyhow!("failed to create tokio runtime: {e}"))?;
    let network = NetworkClient::new()
        .map_err(|e| anyhow::anyhow!("failed to create signed network client: {e}"))?;

    let payload = runtime
        .block_on(network.pull_changes(
            remote_url,
            view_name,
            &local_frontier,
            &identity,
            remote_author_public_key,
            &local.store,
        ))
        .map_err(|e| anyhow::anyhow!("signed pull failed: {e}"))?;

    for change in &payload.changes {
        for atom in &change.atoms {
            match atom {
                Atom::Insert { content_hash, .. } if !local.store.contains_blob(content_hash) => {
                    let bytes = runtime
                        .block_on(network.fetch_blob(remote_url, content_hash))
                        .map_err(|e| anyhow::anyhow!("failed to fetch blob: {e}"))?;
                    let _ = local.store.write_blob(&bytes);
                }
                Atom::Delete { prior_hash, .. } if !local.store.contains_blob(prior_hash) => {
                    let bytes = runtime
                        .block_on(network.fetch_blob(remote_url, prior_hash))
                        .map_err(|e| anyhow::anyhow!("failed to fetch prior blob: {e}"))?;
                    let _ = local.store.write_blob(&bytes);
                }
                _ => {}
            }
        }
        local.graph_add_change(change.clone());
    }

    Ok(payload.view_heads)
}

fn load_active_identity_for_sync() -> anyhow::Result<ArcIdentity> {
    let manager = IdentityManager::init()
        .map_err(|e| anyhow::anyhow!("failed to initialize keyring: {e}"))?;
    let facade = KeyringSessionFacade::new(manager);
    let alias = facade
        .active_alias()
        .map_err(|e| anyhow::anyhow!("failed to read active identity alias: {e}"))?
        .ok_or_else(|| {
            anyhow::anyhow!("no active identity selected; run 'arc auth login' before push/pull")
        })?;
    let passphrase = std::env::var("ARC_KEYRING_PASSPHRASE").map_err(|_| {
        anyhow::anyhow!(
            "ARC_KEYRING_PASSPHRASE is required to unlock identity '{alias}' for signed sync"
        )
    })?;
    facade
        .manager()
        .load(&alias, &passphrase)
        .map_err(|e| anyhow::anyhow!("failed to unlock identity '{alias}': {e}"))
}

fn load_expected_remote_author_key() -> anyhow::Result<[u8; 32]> {
    let raw = std::env::var("ARC_REMOTE_AUTHOR_KEY").map_err(|_| {
        anyhow::anyhow!(
            "ARC_REMOTE_AUTHOR_KEY must be set to the trusted 64-hex remote signing key"
        )
    })?;
    hex_to_blake3(&raw)
}

fn allow_unsigned_sync_fallback() -> bool {
    std::env::var("ARC_ALLOW_UNSIGNED_SYNC_FALLBACK")
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}

/// Minimum blob size that gets its own dedicated progress bar line.
/// Below this threshold, blobs are silently streamed and only the master
/// bytes-uploaded counter is updated.
const HEAVY_BLOB_THRESHOLD: u64 = 5_242_880; // 5 MiB

#[derive(Clone, Default)]
struct FirstErrorSlot {
    inner: Arc<Mutex<Option<anyhow::Error>>>,
}

impl FirstErrorSlot {
    fn capture(&self, error: anyhow::Error) {
        if let Ok(mut slot) = self.inner.lock()
            && slot.is_none()
        {
            *slot = Some(error);
        }
    }

    fn is_set(&self) -> bool {
        self.inner.lock().map(|slot| slot.is_some()).unwrap_or(true)
    }

    fn take(&self) -> Option<anyhow::Error> {
        self.inner.lock().ok().and_then(|mut slot| slot.take())
    }
}

fn validate_blob_sources_parallel(
    local: &Repository,
    blob_hashes: &[Blake3Hash],
) -> anyhow::Result<()> {
    if blob_hashes.is_empty() {
        return Ok(());
    }

    let workers = std::thread::available_parallelism().map(|n| n.get().min(4)).unwrap_or(1);
    if workers <= 1 || blob_hashes.len() <= 1 {
        for hash in blob_hashes {
            let blob_path = local.store.blob_file_path(hash);
            let _ = blob_path.metadata().map_err(|e| {
                anyhow::anyhow!("cannot stat local blob at '{}': {e}", blob_path.display())
            })?;
        }
        return Ok(());
    }

    let inputs: Vec<std::path::PathBuf> =
        blob_hashes.iter().map(|hash| local.store.blob_file_path(hash)).collect();
    let slot = FirstErrorSlot::default();
    let chunk_size = inputs.len().div_ceil(workers).max(1);

    std::thread::scope(|scope| {
        for chunk in inputs.chunks(chunk_size) {
            let slot = slot.clone();
            scope.spawn(move || {
                for path in chunk {
                    if slot.is_set() {
                        return;
                    }
                    if let Err(error) = path.metadata() {
                        slot.capture(anyhow::anyhow!(
                            "cannot stat local blob at '{}': {error}",
                            path.display()
                        ));
                        return;
                    }
                }
            });
        }
    });

    if let Some(error) = slot.take() {
        return Err(error);
    }
    Ok(())
}

/// Stream each blob file to `PUT {remote_url}/blobs/{hex}` without loading it
/// into RAM.  Both 200 (already existed) and 201 (created) are success codes.
///
/// **Progress reporting** — uses a `MultiProgress` layout:
/// - One persistent master bar showing total bytes / bytes-per-second.
/// - Per-blob child bars (inserted above the master) for blobs that exceed
///   [`HEAVY_BLOB_THRESHOLD`].  Each child bar clears itself on completion,
///   keeping the terminal clean during bulk small-blob uploads.
fn upload_blobs(
    client: &reqwest::blocking::Client,
    remote_url: &str,
    local: &Repository,
    blob_hashes: &[Blake3Hash],
    total_bytes: u64,
) -> anyhow::Result<()> {
    validate_blob_sources_parallel(local, blob_hashes)?;

    let mp = indicatif::MultiProgress::new();
    let master_style = indicatif::ProgressStyle::with_template(
        "{bar:40.cyan/blue} {bytes}/{total_bytes} ({bytes_per_sec}) {msg}",
    )
    .unwrap()
    .progress_chars("=>-");
    let master_pb = mp.add(indicatif::ProgressBar::new(total_bytes));
    master_pb.set_style(master_style);
    master_pb.enable_steady_tick(Duration::from_millis(80));

    let n = blob_hashes.len();
    for (idx, hash) in blob_hashes.iter().enumerate() {
        let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
        master_pb.set_message(format!("{}/{n} blobs", idx + 1));
        let blob_path = local.store.blob_file_path(hash);
        let file = std::fs::File::open(&blob_path)
            .map_err(|e| anyhow::anyhow!("cannot open local blob {hex}: {e}"))?;
        let size =
            file.metadata().map_err(|e| anyhow::anyhow!("cannot stat blob {hex}: {e}"))?.len();
        let url = format!("{remote_url}/blobs/{hex}");

        let resp = if size > HEAVY_BLOB_THRESHOLD {
            // Large blob: spawn a per-file progress bar inserted above the master.
            let file_style = indicatif::ProgressStyle::with_template(
                "  {bar:38.yellow/black} {bytes}/{total_bytes} [{msg}]",
            )
            .unwrap()
            .progress_chars("=>-");
            let file_pb = mp.insert_before(&master_pb, indicatif::ProgressBar::new(size));
            file_pb.set_style(file_style);
            file_pb.set_message(format!("{} ...", &hex[..8]));
            let reader = file_pb.wrap_read(file);
            let resp = client
                .put(&url)
                .header("content-length", size)
                .body(reqwest::blocking::Body::sized(reader, size))
                .send()
                .map_err(|e| anyhow::anyhow!("PUT {url} failed: {e}"))?;
            file_pb.finish_and_clear();
            resp
        } else {
            // Small blob: stream directly from disk, no per-file bar.
            client
                .put(&url)
                .header("content-length", size)
                .body(reqwest::blocking::Body::from(file))
                .send()
                .map_err(|e| anyhow::anyhow!("PUT {url} failed: {e}"))?
        };

        if !resp.status().is_success() {
            master_pb.abandon();
            return Err(anyhow::anyhow!(
                "server rejected blob {}: HTTP {}",
                &hex[..8],
                resp.status()
            ));
        }
        master_pb.inc(size);
    }
    master_pb.finish_with_message("Done.");
    Ok(())
}

/// POST the [`DeltaPayload`] to `/sync/:view_name`.
///
/// Handles **409 Conflict** (server missed a blob) with a single retry:
/// re-uploads each missing blob then retries the POST once.  If the server
/// still returns 409 after the retry, the push aborts to prevent a network
/// flood caused by a persistent hash mismatch.
fn post_payload_with_retry(
    client: &reqwest::blocking::Client,
    remote_url: &str,
    view_name: &str,
    payload: &DeltaPayload,
    local: &Repository,
    pb: &indicatif::ProgressBar,
) -> anyhow::Result<SyncResponse> {
    let url = format!("{remote_url}/sync/{view_name}");
    let mut already_retried = false;

    loop {
        let resp = client
            .post(&url)
            .json(payload)
            .send()
            .map_err(|e| anyhow::anyhow!("POST {url} failed: {e}"))?;

        if resp.status() == reqwest::StatusCode::CONFLICT {
            if already_retried {
                // Hard-fail: the missing-blob list did not shrink after one
                // re-upload cycle, indicating a persistent hash mismatch.
                return Err(anyhow::anyhow!(
                    "server persistently reports missing blobs after re-upload \
                     (hash algorithm mismatch?) — aborting to prevent network flood"
                ));
            }
            already_retried = true;

            let missing: Vec<String> = resp
                .json()
                .map_err(|e| anyhow::anyhow!("failed to parse 409 response body: {e}"))?;
            if missing.is_empty() {
                return Err(anyhow::anyhow!(
                    "server returned 409 Conflict with an empty missing-blob list"
                ));
            }
            pb.set_message(format!("Re-uploading {} missing blob(s)...", missing.len()));
            for hex in &missing {
                let hash = hex_to_blake3(hex)?;
                let blob_path = local.store.blob_file_path(&hash);
                let file = std::fs::File::open(&blob_path)
                    .map_err(|e| anyhow::anyhow!("cannot open local blob {hex}: {e}"))?;
                let size = file.metadata()?.len();
                let blob_url = format!("{remote_url}/blobs/{hex}");
                client
                    .put(&blob_url)
                    .header("content-length", size)
                    .body(reqwest::blocking::Body::from(file))
                    .send()
                    .map_err(|e| anyhow::anyhow!("PUT {blob_url} failed: {e}"))?
                    .error_for_status()
                    .map_err(|e| anyhow::anyhow!("server rejected blob re-upload: {e}"))?;
            }
            continue; // retry the POST
        }

        return resp
            .error_for_status()
            .map_err(|e| anyhow::anyhow!("server rejected push: {e}"))?
            .json::<SyncResponse>()
            .map_err(|e| anyhow::anyhow!("failed to deserialize SyncResponse: {e}"));
    }
}

/// Decode a 64-character lowercase hex string into a `Blake3Hash`.
fn hex_to_blake3(hex: &str) -> anyhow::Result<Blake3Hash> {
    if hex.len() != 64 {
        return Err(anyhow::anyhow!(
            "invalid BLAKE3 hash hex length: expected 64, got {}",
            hex.len()
        ));
    }
    let mut bytes = [0u8; 32];
    for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
        let hi = hex_nibble(chunk[0])?;
        let lo = hex_nibble(chunk[1])?;
        bytes[i] = (hi << 4) | lo;
    }
    Ok(bytes)
}

fn hex_nibble(c: u8) -> anyhow::Result<u8> {
    match c {
        b'0'..=b'9' => Ok(c - b'0'),
        b'a'..=b'f' => Ok(c - b'a' + 10),
        b'A'..=b'F' => Ok(c - b'A' + 10),
        _ => Err(anyhow::anyhow!("invalid hex nibble: {c}")),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn test_network_pull() {
        let dir = tempfile::tempdir().unwrap();
        let path_a = dir.path().join("repo_a");
        let path_b = dir.path().join("repo_b");

        // --- Set up Repo A with a file ---
        let mut repo_a = Repository::init(&path_a).unwrap();
        let (author_a, key_a) = arc_store_types::author::test_keypair();
        repo_a.set_identity(author_a, key_a);
        fs::write(path_a.join("a.rs"), "fn a() {}").unwrap();
        repo_a.snap("add a.rs", false).unwrap();

        // --- Init Repo B and pull from A ---
        let mut repo_b = Repository::init(&path_b).unwrap();
        let (author_b, key_b) = arc_store_types::author::test_keypair();
        repo_b.set_identity(author_b, key_b);
        pull(&mut repo_b, path_a.to_str().unwrap(), "main").unwrap();

        // B should now have a.rs on disk.
        assert!(path_b.join("a.rs").exists(), "a.rs must exist in repo B after pull");
        assert_eq!(fs::read_to_string(path_b.join("a.rs")).unwrap(), "fn a() {}");

        // --- Diverge: A adds c.rs, B adds b.rs ---
        fs::write(path_a.join("c.rs"), "fn c() {}").unwrap();
        repo_a.snap("add c.rs", false).unwrap();

        fs::write(path_b.join("b.rs"), "fn b() {}").unwrap();
        repo_b.snap("add b.rs", false).unwrap();

        // --- Pull A into B — disjoint files must commute ---
        pull(&mut repo_b, path_a.to_str().unwrap(), "main").unwrap();

        // B's working directory should have all three files.
        assert!(path_b.join("a.rs").exists(), "a.rs must survive the merge");
        assert!(path_b.join("b.rs").exists(), "b.rs (local to B) must survive the merge");
        assert!(path_b.join("c.rs").exists(), "c.rs (from A) must appear after pull");

        // Verify graph completeness: B's graph should have all of A's changes.
        let view_b = arc_store_view::View::load(&path_b, "main").unwrap();
        assert!(
            view_b.heads.len() >= 2,
            "merged view must have at least 2 heads after divergent pull, got: {}",
            view_b.heads.len()
        );

        // Verify content.
        assert_eq!(fs::read_to_string(path_b.join("a.rs")).unwrap(), "fn a() {}");
        assert_eq!(fs::read_to_string(path_b.join("b.rs")).unwrap(), "fn b() {}");
        assert_eq!(fs::read_to_string(path_b.join("c.rs")).unwrap(), "fn c() {}");
    }

    /// `push_local` round-trip: after pushing A → B, B's view heads must match A's
    /// and B's CAS must contain every head change.
    #[test]
    fn test_push_local() {
        let dir = tempfile::tempdir().unwrap();
        let path_a = dir.path().join("repo_a");
        let path_b = dir.path().join("repo_b");

        // Set up A with one file.
        let mut repo_a = Repository::init(&path_a).unwrap();
        let (author_a, key_a) = arc_store_types::author::test_keypair();
        repo_a.set_identity(author_a, key_a);
        fs::write(path_a.join("widget.rs"), "pub fn widget() {}").unwrap();
        repo_a.snap("add widget.rs", false).unwrap();

        // Initialise an empty B.
        let mut repo_b = Repository::init(&path_b).unwrap();
        let (author_b, key_b) = arc_store_types::author::test_keypair();
        repo_b.set_identity(author_b, key_b);

        // Push A's view to B.
        push(&mut repo_a, path_b.to_str().unwrap(), "main").unwrap();

        // B's view heads must exactly match A's after push.
        let view_a = arc_store_view::View::load(&path_a, "main").unwrap();
        let view_b = arc_store_view::View::load(&path_b, "main").unwrap();
        assert_eq!(view_a.heads, view_b.heads, "pushed view heads must match A's heads");

        // B's CAS must contain every change reachable from A's heads.
        for &head in &view_a.heads {
            assert!(
                repo_b.store.read_change(&head).is_ok(),
                "B's CAS must contain A's head change after push"
            );
        }
    }

    #[test]
    fn first_error_slot_keeps_initial_error() {
        let slot = FirstErrorSlot::default();
        slot.capture(anyhow::anyhow!("first"));
        slot.capture(anyhow::anyhow!("second"));
        let err = slot.take().expect("must keep first error");
        assert!(err.to_string().contains("first"));
    }
}
