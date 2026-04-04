use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::algebra::Blake3Hash;
use crate::store::graph::ChangeGraph;
use crate::store::newtypes::ChangeId;

/// Persistent bisect state stored at `.arc/bisect/state.bin`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BisectState {
    /// Original revset range expression used to initialize this bisect session.
    pub range_expr: String,
    /// Whether we are searching for first good (`true`) or first bad (`false`).
    pub find_good: bool,
    /// Candidate frontier selected by the range expression.
    pub candidates: BTreeSet<ChangeId>,
    /// Per-change tri-state mark.
    pub marks: BTreeMap<ChangeId, BisectMark>,
    /// Current recommended revision to test.
    pub current: Option<ChangeId>,
    /// Session creation time (unix seconds).
    pub started_at_unix: u64,
}

impl BisectState {
    /// Count revisions still marked [`BisectMark::Untested`].
    pub fn untested_count(&self) -> usize {
        self.marks
            .values()
            .filter(|mark| matches!(mark, BisectMark::Untested))
            .count()
    }

    /// Count revisions marked [`BisectMark::Good`].
    pub fn good_count(&self) -> usize {
        self.marks
            .values()
            .filter(|mark| matches!(mark, BisectMark::Good))
            .count()
    }

    /// Count revisions marked [`BisectMark::Bad`].
    pub fn bad_count(&self) -> usize {
        self.marks
            .values()
            .filter(|mark| matches!(mark, BisectMark::Bad))
            .count()
    }
}

/// Tri-state bisect label for a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BisectMark {
    /// Known good.
    Good,
    /// Known bad.
    Bad,
    /// Not yet tested.
    Untested,
}

/// Deterministic midpoint bisect engine for DAGs.
pub struct BisectEngine;

impl BisectEngine {
    /// Create a new bisect state over the given candidate set.
    pub fn start(range_expr: String, candidates: BTreeSet<ChangeId>, find_good: bool) -> BisectState {
        let mut marks = BTreeMap::new();
        for id in &candidates {
            marks.insert(*id, BisectMark::Untested);
        }
        let started_at_unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        BisectState {
            range_expr,
            find_good,
            candidates,
            marks,
            current: None,
            started_at_unix,
        }
    }

    /// Pick next candidate using deterministic topological midpoint.
    pub fn select_next(graph: &ChangeGraph, state: &BisectState) -> Option<ChangeId> {
        let ordered = graph.topological_sort_ids(&state.candidates);
        let untested: Vec<ChangeId> = ordered
            .into_iter()
            .filter(|id| {
                matches!(
                    state.marks.get(id).copied().unwrap_or(BisectMark::Untested),
                    BisectMark::Untested
                )
            })
            .collect();
        if untested.is_empty() {
            None
        } else {
            Some(untested[untested.len() / 2])
        }
    }

    /// Apply a test mark and propagate monotonic constraints over the DAG.
    pub fn mark(
        graph: &ChangeGraph,
        state: &mut BisectState,
        id: ChangeId,
        mark: BisectMark,
    ) -> Result<()> {
        match mark {
            BisectMark::Good => {
                let ancestors = graph.ancestors(&HashSet::from([Blake3Hash::from(id)]));
                for ancestor in ancestors {
                    let ancestor_id = ChangeId::from(ancestor);
                    if state.candidates.contains(&ancestor_id) {
                        set_checked_mark(state, ancestor_id, BisectMark::Good)?;
                    }
                }
            }
            BisectMark::Bad => {
                let mut queue = VecDeque::from([id]);
                let mut visited = BTreeSet::new();
                while let Some(cur) = queue.pop_front() {
                    if !visited.insert(cur) {
                        continue;
                    }
                    if state.candidates.contains(&cur) {
                        set_checked_mark(state, cur, BisectMark::Bad)?;
                    }
                    for child in graph.child_ids(cur) {
                        queue.push_back(child);
                    }
                }
            }
            BisectMark::Untested => {
                state.marks.insert(id, BisectMark::Untested);
            }
        }
        Ok(())
    }
}

fn set_checked_mark(state: &mut BisectState, id: ChangeId, next: BisectMark) -> Result<()> {
    let prev = state.marks.get(&id).copied().unwrap_or(BisectMark::Untested);
    if matches!(prev, BisectMark::Good) && matches!(next, BisectMark::Bad) {
        anyhow::bail!("contradiction: {id} already marked good");
    }
    if matches!(prev, BisectMark::Bad) && matches!(next, BisectMark::Good) {
        anyhow::bail!("contradiction: {id} already marked bad");
    }
    state.marks.insert(id, next);
    Ok(())
}

/// Return canonical path to the bisect state file.
pub fn state_path(shared_root: &Path) -> PathBuf {
    shared_root.join(".arc").join("bisect").join("state.bin")
}

fn reset_marker_path(shared_root: &Path) -> PathBuf {
    shared_root.join(".arc").join("bisect").join("reset.marker")
}

/// Load persisted bisect state from disk.
pub fn load_state(shared_root: &Path) -> Result<Option<BisectState>> {
    if reset_marker_path(shared_root).exists() {
        return Ok(None);
    }
    let primary = state_path(shared_root);
    let staged_backup = primary.with_extension("bak.new");
    let backup = primary.with_extension("bak");
    let path = if primary.exists() {
        primary
    } else if staged_backup.exists() {
        staged_backup
    } else if backup.exists() {
        backup
    } else {
        return Ok(None);
    };
    let bytes = fs::read(&path)
        .with_context(|| format!("failed to read bisect state '{}':", path.display()))?;
    let state = bincode::deserialize::<BisectState>(&bytes)
        .with_context(|| format!("failed to decode bisect state '{}':", path.display()))?;
    Ok(Some(state))
}

/// Persist bisect state using crash-consistent atomic replacement.
pub fn save_state(shared_root: &Path, state: &BisectState) -> Result<()> {
    let path = state_path(shared_root);
    let bytes = bincode::serialize(state).context("failed to serialize bisect state")?;
    atomic_write_bytes(&path, &bytes)?;
    let marker = reset_marker_path(shared_root);
    if marker.exists() {
        fs::remove_file(&marker).with_context(|| {
            format!("failed to remove bisect reset marker '{}':", marker.display())
        })?;
    }
    Ok(())
}

/// Remove persisted bisect state, if present.
pub fn clear_state(shared_root: &Path) -> Result<()> {
    let marker = reset_marker_path(shared_root);
    atomic_write_bytes(&marker, b"reset")?;

    let path = state_path(shared_root);
    for candidate in [path.clone(), path.with_extension("bak"), path.with_extension("bak.new")] {
        if candidate.exists() {
            fs::remove_file(&candidate).with_context(|| {
                format!("failed to remove bisect state '{}':", candidate.display())
            })?;
        }
    }
    Ok(())
}

fn atomic_write_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent)?;

    let tmp_name = format!(
        ".{}.tmp-{}-{}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("bisect"),
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let tmp_path = parent.join(tmp_name);

    {
        let mut file = File::create(&tmp_path)
            .with_context(|| format!("failed to create temp file {}", tmp_path.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("failed to write temp file {}", tmp_path.display()))?;
        file.sync_all()
            .with_context(|| format!("failed to fsync temp file {}", tmp_path.display()))?;
    }

    #[cfg(windows)]
    {
        let backup_path = path.with_extension("bak");
        let staged_backup_path = path.with_extension("bak.new");
        if staged_backup_path.exists() {
            fs::remove_file(&staged_backup_path).with_context(|| {
                format!(
                    "failed to remove stale staged backup {}",
                    staged_backup_path.display()
                )
            })?;
        }
        if path.exists() {
            fs::rename(path, &staged_backup_path).with_context(|| {
                format!(
                    "failed to rotate existing target {} -> {}",
                    path.display(),
                    staged_backup_path.display()
                )
            })?;
        }
        if let Err(err) = fs::rename(&tmp_path, path) {
            if staged_backup_path.exists() {
                let _ = fs::rename(&staged_backup_path, path);
            }
            return Err(err).with_context(|| {
                format!(
                    "failed to atomically rename {} -> {}",
                    tmp_path.display(),
                    path.display()
                )
            });
        }
        if staged_backup_path.exists() {
            if backup_path.exists() {
                fs::remove_file(&backup_path).with_context(|| {
                    format!("failed to replace previous backup {}", backup_path.display())
                })?;
            }
            fs::rename(&staged_backup_path, &backup_path).with_context(|| {
                format!(
                    "failed to finalize backup {} -> {}",
                    staged_backup_path.display(),
                    backup_path.display()
                )
            })?;
        }
    }

    #[cfg(not(windows))]
    {
        fs::rename(&tmp_path, path).with_context(|| {
            format!(
                "failed to atomically rename {} -> {}",
                tmp_path.display(),
                path.display()
            )
        })?;
    }

    if let Ok(dir) = File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::author;
    use crate::store::change::Change;

    fn make_change(deps: HashSet<Blake3Hash>, label: &str) -> Change {
        let (author, signing_key) = author::test_keypair();
        let content_hash: [u8; 32] = *blake3::hash(label.as_bytes()).as_bytes();
        Change::new(
            deps,
            vec![crate::algebra::Atom::Insert {
                at: vec![label.to_string()],
                content_hash,
            }],
            "test",
            author,
            &signing_key,
        )
    }

    fn small_chain() -> (ChangeGraph, ChangeId, ChangeId, ChangeId) {
        let mut g = ChangeGraph::new();
        let a = make_change(HashSet::new(), "a");
        let b = make_change(HashSet::from([a.id]), "b");
        let c = make_change(HashSet::from([b.id]), "c");
        g.add_change(a.clone());
        g.add_change(b.clone());
        g.add_change(c.clone());
        (g, ChangeId::from(a.id), ChangeId::from(b.id), ChangeId::from(c.id))
    }

    #[test]
    fn midpoint_is_deterministic() {
        let (g, a, b, c) = small_chain();
        let state = BisectEngine::start(
            "ancestors(@)".to_string(),
            BTreeSet::from([a, b, c]),
            false,
        );
        assert_eq!(BisectEngine::select_next(&g, &state), Some(b));
    }

    #[test]
    fn mark_good_propagates_to_ancestors() {
        let (g, a, b, c) = small_chain();
        let mut state = BisectEngine::start(
            "ancestors(@)".to_string(),
            BTreeSet::from([a, b, c]),
            false,
        );
        BisectEngine::mark(&g, &mut state, c, BisectMark::Good).unwrap();
        assert!(matches!(state.marks.get(&a), Some(BisectMark::Good)));
        assert!(matches!(state.marks.get(&b), Some(BisectMark::Good)));
        assert!(matches!(state.marks.get(&c), Some(BisectMark::Good)));
    }

    #[test]
    fn mark_bad_propagates_to_descendants() {
        let (g, a, b, c) = small_chain();
        let mut state = BisectEngine::start(
            "ancestors(@)".to_string(),
            BTreeSet::from([a, b, c]),
            false,
        );
        BisectEngine::mark(&g, &mut state, a, BisectMark::Bad).unwrap();
        assert!(matches!(state.marks.get(&a), Some(BisectMark::Bad)));
        assert!(matches!(state.marks.get(&b), Some(BisectMark::Bad)));
        assert!(matches!(state.marks.get(&c), Some(BisectMark::Bad)));
    }

    #[test]
    fn persistence_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let shared = tmp.path();
        let (_, a, b, c) = small_chain();
        let mut state = BisectEngine::start(
            "ancestors(@)".to_string(),
            BTreeSet::from([a, b, c]),
            false,
        );
        state.current = Some(b);
        save_state(shared, &state).unwrap();
        let loaded = load_state(shared).unwrap().unwrap();
        assert_eq!(loaded.current, Some(b));
        assert_eq!(loaded.candidates, state.candidates);
    }
}