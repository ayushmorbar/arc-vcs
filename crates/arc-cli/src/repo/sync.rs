use std::collections::HashMap;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::policy_gate::{ArcPolicy, Ast, Evaluator, PolicyError};
use arc_algebra_types::Atom;
use arc_algebra_types::Blake3Hash;
use arc_algebra_types::SpacetimeCoordinate;
use arc_change::Change;
use arc_net::sync::client::NativeSyncClient;
use arc_net::sync::protocol::{CasWireBlock, SyncProtocol, compute_missing_hashes};
use arc_store_view::View;

use super::core::*;

impl Repository {
    fn persist_policy_error_payload(
        &self,
        error: &PolicyError,
        mount_path: &str,
        view_name: &str,
        view_heads: &[String],
    ) -> anyhow::Result<()> {
        let created_at =
            SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let payload = serde_json::json!({
            "mount_path": mount_path,
            "view_name": view_name,
            "view_heads": view_heads,
            "created_at": created_at,
            "error": error.to_mcp_payload(),
        });
        let path = self.shared_root.join(".arc").join("ai").join("last_policy_error.json");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| anyhow::anyhow!("failed to create AI metadata dir: {e}"))?;
        }
        let json = serde_json::to_vec_pretty(&payload)
            .map_err(|e| anyhow::anyhow!("failed to encode policy MCP payload: {e}"))?;
        fs::write(&path, json)
            .map_err(|e| anyhow::anyhow!("failed to persist policy MCP payload: {e}"))?;
        Ok(())
    }

    /// Perform native transport sync and enforce semantic policy before any
    /// incoming change is written into local CAS/graph state.
    pub fn sync_native_with_semantic_gate(
        &mut self,
        address: &str,
        auth_token: Option<String>,
    ) -> anyhow::Result<usize> {
        let view_name = self.current_view_name()?;
        self.hydrate(&view_name)?;
        let current_view = View::load(&self.shared_root, &view_name)
            .map_err(|e| anyhow::anyhow!("failed to load view '{view_name}': {e}"))?;
        let local_frontier: Vec<blake3::Hash> =
            current_view.heads.iter().copied().map(blake3::Hash::from).collect();

        let mut view_heads: Vec<String> = current_view
            .heads
            .iter()
            .map(|h: &Blake3Hash| h.iter().map(|b| format!("{b:02x}")).collect::<String>())
            .collect();
        view_heads.sort();

        let policy_path = self.shared_root.join(".arc").join("arc.policy.json");
        let policy = ArcPolicy::load_from_path(&policy_path)
            .map_err(|e| anyhow::anyhow!("policy load failed before native sync: {e}"))?;
        let evaluator = policy.default_evaluator();
        let local_ast = Ast::default();

        let client = NativeSyncClient::new(address.to_string(), auth_token);
        let runtime = tokio::runtime::Runtime::new()
            .map_err(|e| anyhow::anyhow!("failed to start async runtime: {e}"))?;
        let remote_frontier = runtime
            .block_on(client.exchange_frontiers(local_frontier.clone()))
            .map_err(|e| anyhow::anyhow!("frontier exchange failed: {e}"))?;

        let missing_hashes = compute_missing_hashes(&self.store, &local_frontier, &remote_frontier)
            .map_err(|e| anyhow::anyhow!("failed to compute missing frontier closure: {e}"))?;

        if missing_hashes.is_empty() {
            return Ok(0);
        }

        let packed_blocks = runtime
            .block_on(client.fetch_cas_blocks(&missing_hashes))
            .map_err(|e| anyhow::anyhow!("CAS transfer failed: {e}"))?;

        // Keep all downloaded blocks transient until semantic policy accepts
        // the complete incoming delta.
        let blocks: Vec<CasWireBlock> = bincode::deserialize(&packed_blocks)
            .map_err(|e| anyhow::anyhow!("failed to decode downloaded CAS blocks: {e}"))?;
        let mut decoded_changes = Vec::with_capacity(blocks.len());

        for block in &blocks {
            let computed = blake3::hash(&block.bytes);
            if computed.as_bytes() != &block.hash {
                return Err(anyhow::anyhow!(
                    "downloaded CAS block hash mismatch for {}",
                    arc_store_types::newtypes::ChangeId::from(block.hash).to_hex()
                ));
            }

            let change: Change = bincode::deserialize(&block.bytes)
                .map_err(|e| anyhow::anyhow!("failed to decode downloaded change: {e}"))?;
            if change.id != block.hash {
                return Err(anyhow::anyhow!(
                    "downloaded change id mismatch for {}",
                    arc_store_types::newtypes::ChangeId::from(block.hash).to_hex()
                ));
            }
            if !change.verify_signature() {
                return Err(anyhow::anyhow!(
                    "downloaded change failed cryptographic verification for {}",
                    arc_store_types::newtypes::ChangeId::from(change.id).to_hex()
                ));
            }
            decoded_changes.push(change);
        }

        let mut incoming_atoms = Vec::new();
        for change in &decoded_changes {
            incoming_atoms.extend(change.atoms.clone());
        }

        if let Err(error) = evaluator.evaluate_delta_impact(&local_ast, &incoming_atoms) {
            let _ = self.persist_policy_error_payload(&error, address, &view_name, &view_heads);
            return Err(match error {
                PolicyError::SignatureMismatch {
                    broken_functions,
                    old_signature,
                    new_signature,
                } => anyhow::anyhow!(
                    "policy gate rejected incoming sync delta from '{}': broken functions [{}]; old signature [{}]; new signature [{}]. \
                     Transient CAS buffer dropped. Run 'arc ai resolve' to generate a Lensed Ghost Node.",
                    address,
                    broken_functions.join(", "),
                    old_signature,
                    new_signature
                ),
                other => anyhow::anyhow!(
                    "policy gate rejected incoming sync delta from '{}': {}. \
                     Transient CAS buffer dropped. Run 'arc ai resolve'.",
                    address,
                    other
                ),
            });
        }

        for (block, change) in blocks.iter().zip(decoded_changes.iter()) {
            self.store
                .write_change_bytes(
                    arc_store_types::newtypes::ChangeId::from(block.hash),
                    &block.bytes,
                )
                .map_err(|e| anyhow::anyhow!("failed to persist downloaded CAS block: {e}"))?;
            self.graph_add_change(change.clone());
        }

        let remote_heads: std::collections::HashSet<Blake3Hash> =
            remote_frontier.iter().map(|h| *h.as_bytes()).collect();
        self.merge_heads(&remote_heads)?;
        Ok(blocks.len())
    }

    // ------------------------------------------------------------------
    // Remotes
    // ------------------------------------------------------------------

    /// Store a named remote URL alias in `.arc/config.json`.
    ///
    /// If a remote with the same name already exists it is overwritten,
    /// making this operation idempotent.
    pub fn add_remote(&self, name: &str, url: &str) -> anyhow::Result<()> {
        let mut config = self.read_config()?;
        config.remotes.insert(name.to_string(), url.to_string());
        self.write_config(&config)
    }

    /// Return all configured remote aliases.
    pub fn list_remotes(&self) -> anyhow::Result<HashMap<String, String>> {
        Ok(self.read_config()?.remotes)
    }

    /// Remove a named remote alias from `.arc/config.json`.
    ///
    /// Returns an actionable error if the remote does not exist.
    pub fn remove_remote(&self, name: &str) -> anyhow::Result<()> {
        let mut config = self.read_config()?;
        if config.remotes.remove(name).is_none() {
            anyhow::bail!(
                "Remote '{}' does not exist. Use 'arc remote list' to see available remotes.",
                name
            );
        }
        self.write_config(&config)
    }

    // ------------------------------------------------------------------
    // Mount algebra
    // ------------------------------------------------------------------

    /// Clone or update all mounted sub-repositories declared in the current view.
    ///
    /// For each `ARC_MOUNT:` token in the materialized state:
    /// * If the mount directory has no `.arc/` sub-directory, the sub-repository
    ///   is initialised and the target view is fetched via the internal sync API.
    /// * If `.arc/` already exists, the repository is opened and the view is
    ///   fetched to pick up new changes before switching.
    ///
    /// A progress spinner is shown for the full sync pass.
    pub fn mount_sync(&mut self) -> anyhow::Result<()> {
        let view_name = self.current_view_name()?;
        self.hydrate(&view_name)?;
        let state = self.materialize(&view_name)?;
        let current_view = View::load(&self.shared_root, &view_name)
            .map_err(|e| anyhow::anyhow!("failed to load view '{view_name}': {e}"))?;
        let mut view_heads: Vec<String> = current_view
            .heads
            .iter()
            .map(|h: &Blake3Hash| h.iter().map(|b| format!("{b:02x}")).collect::<String>())
            .collect();
        view_heads.sort();
        let policy_path = self.shared_root.join(".arc").join("arc.policy.json");
        let policy = ArcPolicy::load_from_path(&policy_path)
            .map_err(|e| anyhow::anyhow!("policy load failed before sync gate evaluation: {e}"))?;
        let evaluator = policy.default_evaluator();
        let local_ast = Ast::default();

        enum MountSpec {
            Coordinate(SpacetimeCoordinate),
            Legacy { url: String, target: String },
        }

        // Collect all ARC_MOUNT: entries.
        let mounts: Vec<(String, MountSpec)> = state
            .iter()
            .filter_map(|(key, value)| {
                if key.len() == 2 && key[0] == "file" && value.starts_with(b"ARC_MOUNT:") {
                    let info = std::str::from_utf8(value).ok()?.strip_prefix("ARC_MOUNT:")?;
                    if let Ok(coord) = SpacetimeCoordinate::from_uri(info) {
                        Some((key[1].clone(), MountSpec::Coordinate(coord)))
                    } else {
                        let (url, tgt) = info.split_once('|')?;
                        Some((
                            key[1].clone(),
                            MountSpec::Legacy { url: url.to_string(), target: tgt.to_string() },
                        ))
                    }
                } else {
                    None
                }
            })
            .collect();

        if mounts.is_empty() {
            println!("No mounts declared in current view.");
            return Ok(());
        }

        let spinner =
            crate::progress::Progress::spinner(format!("Syncing {} mount(s)...", mounts.len()));

        for (path, spec) in &mounts {
            let mount_dir = self.work_root.join(path);
            let arc_sub = mount_dir.join(".arc");

            let incoming_atoms = match spec {
                MountSpec::Coordinate(coord) => vec![Atom::Mount {
                    path: vec!["file".to_string(), path.clone()],
                    coordinate: coord.clone(),
                }],
                MountSpec::Legacy { url, target } => {
                    let synthetic_coord = SpacetimeCoordinate {
                        namespace: "legacy".to_string(),
                        repo: target.clone(),
                        hash: blake3::hash(format!("{url}|{target}").as_bytes()),
                    };
                    vec![Atom::Mount {
                        path: vec!["file".to_string(), path.clone()],
                        coordinate: synthetic_coord,
                    }]
                }
            };

            if let Err(e) = evaluator.evaluate_delta_impact(&local_ast, &incoming_atoms) {
                let _ = self.persist_policy_error_payload(&e, path, &view_name, &view_heads);
                return Err(match e {
                    PolicyError::SignatureMismatch {
                        broken_functions,
                        old_signature,
                        new_signature,
                    } => anyhow::anyhow!(
                        "policy gate rejected incoming sync delta for mount '{}': broken functions [{}]; old signature [{}]; new signature [{}]. \
                         Generate a Lensed Ghost Node and re-run sync.",
                        path,
                        broken_functions.join(", "),
                        old_signature,
                        new_signature
                    ),
                    PolicyError::MissingDependency { dependency } => anyhow::anyhow!(
                        "policy gate rejected incoming sync delta for mount '{}': missing dependency '{}'. \
                         Reconcile the foreign DAG frontier before retrying sync.",
                        path,
                        dependency
                    ),
                    other => anyhow::anyhow!(
                        "policy gate rejected incoming sync delta for mount '{}': {}",
                        path,
                        other
                    ),
                });
            }

            match spec {
                MountSpec::Coordinate(coord) => {
                    spinner.set_message(format!(
                        "Cannot sync mount '{path}' at {}...",
                        coord.to_uri()
                    ));
                    anyhow::bail!(
                        "mount sync for coordinate '{}' is scaffold-only and not implemented yet",
                        coord.to_uri()
                    );
                }
                MountSpec::Legacy { url, target } => {
                    spinner.set_message(format!("Syncing mount '{path}' from {url}@{target}..."));
                    let mut sub_repo = if arc_sub.exists() {
                        Repository::open(&mount_dir)
                            .map_err(|e| anyhow::anyhow!("failed to open mount '{}': {e}", path))?
                    } else {
                        fs::create_dir_all(&mount_dir).map_err(|e| {
                            anyhow::anyhow!("failed to create mount dir '{}': {e}", path)
                        })?;
                        Repository::init(&mount_dir)
                            .map_err(|e| anyhow::anyhow!("failed to init mount '{}': {e}", path))?
                    };
                    crate::sync::fetch(&mut sub_repo, url, target)
                        .map_err(|e| anyhow::anyhow!("fetch failed for mount '{}': {e}", path))?;
                    sub_repo.switch_view(target).map_err(|e| {
                        anyhow::anyhow!("switch_view failed for mount '{}': {e}", path)
                    })?;
                }
            }
        }

        spinner.finish_with_message(format!("Synced {} mount(s).", mounts.len()));
        Ok(())
    }
}
