use std::collections::HashSet;
use std::fs;
use std::path::Path;

use arc_algebra::apply::MaterializedState;
use arc_algebra_types::{Atom, Blake3Hash};
use arc_change::Change;
use arc_lang::ast::LanguagePlugin;
use arc_lang::ast::rust_plugin::RustPlugin;
use arc_store_view::View;
use gix_features::parallel;

use super::core::*;
use crate::store_compat::ObjectStoreChangeExt;

impl Repository {
    /// Snapshot the current working directory into an implicit working-copy
    /// change at the tip of the current view.
    ///
    /// Returns `Ok(false)` when there is nothing to snapshot.
    pub fn snapshot(&mut self) -> anyhow::Result<bool> {
        self.acquire_lock()?;
        let view_name = self.current_view_name()?;
        self.hydrate(&view_name)?;
        let state = self.materialize(&view_name)?;

        let atoms = self.compute_working_directory_delta(&state)?;
        if atoms.is_empty() {
            return Ok(false);
        }

        let mut view = View::load(&self.shared_root, &view_name)
            .map_err(|e| anyhow::anyhow!("failed to load view '{view_name}': {e}"))?;
        let before_heads = view.heads.clone();
        let (author, signing_key) = self.signing_identity()?;

        self.write_blob_atoms(&atoms)?;
        let change = Change::new(
            view.heads.clone(),
            atoms,
            "snapshot working copy",
            author.clone(),
            signing_key,
        );

        self.store.write_change(&change).map_err(|e| anyhow::anyhow!("CAS write error: {e}"))?;
        self.graph_add_change(change.clone());

        view.heads = HashSet::from([change.id]);
        view.save(&self.shared_root).map_err(|e| anyhow::anyhow!("failed to save view: {e}"))?;

        self.log_operation(
            "snapshot working copy",
            &view_name,
            before_heads,
            HashSet::from([change.id]),
        )?;

        Ok(true)
    }

    /// Finalize the current implicit working-copy change with user-facing
    /// commit metadata.
    pub fn finalize_snapshot(&mut self, message: &str) -> anyhow::Result<Blake3Hash> {
        self.acquire_lock()?;
        let view_name = self.current_view_name()?;
        self.hydrate(&view_name)?;

        let mut view = View::load(&self.shared_root, &view_name)
            .map_err(|e| anyhow::anyhow!("failed to load view '{view_name}': {e}"))?;
        if view.heads.len() != 1 {
            anyhow::bail!(
                "finalize_snapshot requires exactly one head; view '{}' has {} heads",
                view_name,
                view.heads.len()
            );
        }

        let old_head = *view.heads.iter().next().unwrap();
        let old_change = self
            .graph
            .load()
            .get(&old_head)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("change {} missing from graph", _hex(&old_head)))?;

        let (author, signing_key) = self.signing_identity()?;
        let new_change = Change::new(
            old_change.deps.clone(),
            old_change.atoms.clone(),
            message,
            author.clone(),
            signing_key,
        );

        self.store
            .write_change(&new_change)
            .map_err(|e| anyhow::anyhow!("CAS write error: {e}"))?;
        self.graph_add_change(new_change.clone());
        let _ = self.try_embed_change(&new_change);

        view.heads = HashSet::from([new_change.id]);
        view.save(&self.shared_root).map_err(|e| anyhow::anyhow!("failed to save view: {e}"))?;

        self.log_operation(
            "snap",
            &view_name,
            HashSet::from([old_head]),
            HashSet::from([new_change.id]),
        )?;

        Ok(new_change.id)
    }

    /// Fork the next empty implicit working-copy change on top of the current
    /// finalized commit.
    pub fn fork_empty_snapshot(&mut self) -> anyhow::Result<Blake3Hash> {
        self.acquire_lock()?;
        let view_name = self.current_view_name()?;
        self.hydrate(&view_name)?;

        let mut view = View::load(&self.shared_root, &view_name)
            .map_err(|e| anyhow::anyhow!("failed to load view '{view_name}': {e}"))?;
        let before_heads = view.heads.clone();
        let (author, signing_key) = self.signing_identity()?;

        let wc_change = Change::new(
            view.heads.clone(),
            Vec::new(),
            "snapshot working copy",
            author.clone(),
            signing_key,
        );

        self.store.write_change(&wc_change).map_err(|e| anyhow::anyhow!("CAS write error: {e}"))?;
        self.graph_add_change(wc_change.clone());

        view.heads = HashSet::from([wc_change.id]);
        view.save(&self.shared_root).map_err(|e| anyhow::anyhow!("failed to save view: {e}"))?;

        self.log_operation(
            "snapshot working copy",
            &view_name,
            before_heads,
            HashSet::from([wc_change.id]),
        )?;

        Ok(wc_change.id)
    }

    /// Scan the working directory, diff against the materialized history,
    /// and create a new semantic `Change`.
    ///
    /// Returns `Some(change_id)` if a change was created, or `None` if
    /// the working directory matches the materialized state exactly.
    ///
    /// Each file is decomposed into top-level AST items via `diff()`.
    /// The resulting atoms are prefixed with `["file", filepath]` so that
    /// `unparse()` can later reconstruct source per file.
    pub fn snap(&mut self, message: &str, interactive: bool) -> anyhow::Result<Option<Blake3Hash>> {
        // Guard: refuse to snap while a diffedit is in progress.
        let diffedit_lock = self.shared_root.join(".arc").join("diffedit_target");
        if diffedit_lock.exists() {
            anyhow::bail!(
                "A diffedit is in progress. Run 'arc diffedit --apply' to finish, \
                 or 'arc diffedit --abort' to cancel."
            );
        }
        // State Lock: refuse to snap while an AI change is pending approval.
        if crate::ai_pending::has_pending_ai(&self.shared_root) {
            anyhow::bail!(
                "An AI change is pending approval.\n\
                 Run 'arc ai approve' to sign and commit it, \
                 or delete '.arc/ai/pending.json' to discard it."
            );
        }
        self.run_hook("pre-snap")?;
        self.acquire_lock()?;
        let view_name = self.current_view_name()?;
        tracing::info!(view = %view_name, message, "snap started");
        self.hydrate(&view_name)?;
        let state = self.materialize(&view_name)?;

        // Compute every atom that would represent the current working-directory
        // delta relative to the materialized state.
        let raw_atoms = self.compute_working_directory_delta(&state)?;

        if raw_atoms.is_empty() {
            return Ok(None);
        }

        // Interactive staging: filter atoms the user does not want to stage.
        // Deletion / directory atoms are always kept to avoid ghost-file state.
        let all_atoms: Vec<Atom> = if interactive {
            let mut last_file: Option<String> = None;
            select_atoms_interactively(raw_atoms, |filepath, label| {
                use std::io::Write;

                if last_file.as_deref() != Some(filepath) {
                    println!("-- {filepath} --");
                    last_file = Some(filepath.to_string());
                }
                print!("  {label}\n  Stage this change? [y/N] ");
                std::io::stdout().flush().ok();
                let mut line = String::new();
                std::io::stdin().read_line(&mut line).ok();
                line.trim().eq_ignore_ascii_case("y")
            })
        } else {
            raw_atoms
        };

        // Strip the interactive-filtered result: if nothing left after filtering
        // file-AST atoms, and the only remaining atoms are non-AST (dirs etc.),
        // check whether there are any real changes.
        let has_file_change = all_atoms.iter().any(|a| {
            a.paths().first().and_then(|p| p.first()).map(|s| s == "file").unwrap_or(false)
                || matches!(a, Atom::Directory { .. })
                || matches!(a, Atom::Delete { at, .. } if at.first().map(|s| s == "dir").unwrap_or(false))
        });

        if !has_file_change && all_atoms.is_empty() {
            return Ok(None);
        }

        if all_atoms.is_empty() {
            return Ok(None);
        }

        let mut view = View::load(&self.shared_root, &view_name)
            .map_err(|e| anyhow::anyhow!("failed to load view '{view_name}': {e}"))?;

        let (author, signing_key) = self.signing_identity()?;

        // Persist raw bytes for every Atom::Blob before committing the change.
        self.write_blob_atoms(&all_atoms)?;

        let change =
            Change::new(view.heads.clone(), all_atoms, message, author.clone(), signing_key);
        self.store.write_change(&change).map_err(|e| anyhow::anyhow!("CAS write error: {e}"))?;
        self.graph_add_change(change.clone());

        // Update the semantic intent index (no-op if index not yet initialised).
        let _ = self.try_embed_change(&change);

        // Capture the current frontier before advancing it.
        let before_heads = view.heads.clone();

        // Advance the frontier: new change becomes the sole head.
        view.heads = HashSet::from([change.id]);
        view.save(&self.shared_root).map_err(|e| anyhow::anyhow!("failed to save view: {e}"))?;

        // Record the completed snap in the spacetime log.
        self.log_operation("snap", &view_name, before_heads, HashSet::from([change.id]))?;

        tracing::info!(change_id = ?change.id, "snap complete");
        Ok(Some(change.id))
    }

    /// Compute every [`Atom`] that represents the difference between the
    /// materialized `state` and the current working directory.
    ///
    /// This is the pure-computation core shared by [`snap`] and [`status`].
    /// It never touches the CAS, the graph, or any view file.
    pub(super) fn compute_working_directory_delta(
        &self,
        state: &MaterializedState,
    ) -> anyhow::Result<Vec<Atom>> {
        let plugin = RustPlugin::new();
        let sparse_matcher = sparse_matcher_for_root(&self.work_root);
        let arcignore = load_arcignore(&self.work_root);
        let rs_files = collect_rs_files(&self.work_root, &arcignore)?;
        let mut atoms: Vec<Atom> = Vec::new();

        for filepath in &rs_files {
            if !sparse_matcher.matches_file_path(filepath) {
                continue;
            }
            let new_src = fs::read_to_string(self.work_root.join(filepath))?;
            let old_src = plugin.unparse(state, filepath).unwrap_or_default();
            if old_src == new_src {
                continue;
            }
            let ast_atoms = plugin
                .diff(&old_src, &new_src, &self.store)
                .map_err(|e| anyhow::anyhow!("diff error for {filepath}: {e}"))?;
            if ast_atoms.is_empty() {
                continue;
            }
            for atom in ast_atoms {
                atoms.push(prefix_atom_path(atom, filepath));
            }
        }

        // -- Pass 2: Non-Rust files - parallel BLAKE3 blob diff
        let all_files = collect_all_files(&self.work_root, &arcignore)?;
        let tracked_files: HashSet<String> =
            state.keys().filter(|k| k.len() == 2 && k[0] == "file").map(|k| k[1].clone()).collect();
        let non_rs_files: Vec<String> = all_files
            .into_iter()
            .filter(|f| sparse_matcher.matches_file_path(f))
            .filter(|f| !f.ends_with(".rs"))
            .filter(|f| tracked_files.contains(f.as_str()) || !is_implicitly_ignored(Path::new(f)))
            .collect();
        let work_root: &std::path::Path = &self.work_root;
        let blob_atoms = parallel::in_parallel(
            non_rs_files.into_iter(),
            None,
            |_| blake3::Hasher::new(),
            |filepath: String, hasher: &mut blake3::Hasher| -> anyhow::Result<Option<Atom>> {
                let bytes = fs::read(work_root.join(&filepath))
                    .map_err(|e| anyhow::anyhow!("failed to read '{filepath}': {e}"))?;
                hasher.reset();
                hasher.update(&bytes);
                let new_hash: Blake3Hash = *hasher.finalize().as_bytes();
                let path_key = vec!["file".to_string(), filepath.clone()];
                if let Some(existing) = state.get(&path_key)
                    && existing.starts_with(b"ARC_BLOB_REF:")
                    && existing.len() >= 45
                {
                    let old_hash: Blake3Hash = existing[13..45].try_into().unwrap_or([0u8; 32]);
                    if old_hash == new_hash {
                        return Ok(None);
                    }
                }
                Ok(Some(Atom::Blob {
                    path: filepath,
                    hash: new_hash.into(),
                    size: bytes.len() as u64,
                }))
            },
            BlobAtomReducer::new(),
        )?;
        atoms.extend(blob_atoms);

        // Deleted files.
        let state_filepaths = extract_filepaths_from_state(state);
        for filepath in &state_filepaths {
            // Sparse Safety Law: do not emit Delete for files hidden by sparse cone.
            if !sparse_matcher.matches_file_path(filepath) {
                continue;
            }
            if !self.work_root.join(filepath).exists() {
                let prefix = ["file".to_string(), filepath.clone()];
                for key in state.keys() {
                    // >= covers blob keys (len==2) as well as AST sub-keys (len>2).
                    if key.len() >= prefix.len() && key[..prefix.len()] == prefix[..] {
                        let prior_bytes = state.get(key).cloned().unwrap_or_default();
                        let prior_hash = self.store.write_blob(&prior_bytes).map_err(|e| {
                            anyhow::anyhow!("CAS write error for deleted file: {e}")
                        })?;
                        atoms.push(Atom::Delete { at: key.clone(), prior_hash });
                    }
                }
            }
        }

        // New / removed empty directories.
        let dir_key = |rel: &str| vec!["dir".to_string(), rel.to_string()];
        let existing_dirs: HashSet<String> = state
            .keys()
            .filter(|k| k.len() == 2 && k[0] == "dir")
            .filter(|k| sparse_matcher.matches_file_path(&k[1]))
            .map(|k| k[1].clone())
            .collect();
        for rel_dir in collect_empty_dirs(&self.work_root, &arcignore)? {
            if !sparse_matcher.matches_file_path(&rel_dir) {
                continue;
            }
            if !existing_dirs.contains(&rel_dir) {
                atoms.push(Atom::Directory { path: dir_key(&rel_dir) });
            }
        }
        for rel_dir in &existing_dirs {
            if !self.work_root.join(rel_dir).exists() {
                let key = dir_key(rel_dir);
                let prior_bytes = state.get(&key).cloned().unwrap_or_default();
                let prior_hash = self
                    .store
                    .write_blob(&prior_bytes)
                    .map_err(|e| anyhow::anyhow!("CAS write error for deleted dir: {e}"))?;
                atoms.push(Atom::Delete { at: key, prior_hash });
            }
        }

        Ok(atoms)
    }
}
