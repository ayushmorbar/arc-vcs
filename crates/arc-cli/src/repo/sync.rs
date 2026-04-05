use std::collections::HashMap;
use std::fs;

use super::core::*;

impl Repository {
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

        // Collect all ARC_MOUNT: entries.
        let mounts: Vec<(String, String, String)> = state
            .iter()
            .filter_map(|(key, value)| {
                if key.len() == 2 && key[0] == "file" && value.starts_with(b"ARC_MOUNT:") {
                    let info = std::str::from_utf8(value)
                        .ok()?
                        .strip_prefix("ARC_MOUNT:")?;
                    let (url, tgt) = info.split_once('|')?;
                    Some((key[1].clone(), url.to_string(), tgt.to_string()))
                } else {
                    None
                }
            })
            .collect();

        if mounts.is_empty() {
            println!("No mounts declared in current view.");
            return Ok(());
        }

        let spinner = crate::progress::Progress::spinner(format!("Syncing {} mount(s)...", mounts.len()));

        for (path, url, target) in &mounts {
            spinner.set_message(format!("Syncing mount '{path}' from {url}@{target}..."));
            let mount_dir = self.work_root.join(path);
            let arc_sub = mount_dir.join(".arc");
            let mut sub_repo = if arc_sub.exists() {
                Repository::open(&mount_dir)
                    .map_err(|e| anyhow::anyhow!("failed to open mount '{}': {e}", path))?
            } else {
                fs::create_dir_all(&mount_dir)
                    .map_err(|e| anyhow::anyhow!("failed to create mount dir '{}': {e}", path))?;
                Repository::init(&mount_dir)
                    .map_err(|e| anyhow::anyhow!("failed to init mount '{}': {e}", path))?
            };
            crate::sync::fetch(&mut sub_repo, url, target)
                .map_err(|e| anyhow::anyhow!("fetch failed for mount '{}': {e}", path))?;
            sub_repo
                .switch_view(target)
                .map_err(|e| anyhow::anyhow!("switch_view failed for mount '{}': {e}", path))?;
        }

        spinner.finish_with_message(format!("Synced {} mount(s).", mounts.len()));
        Ok(())
    }
}
