// TODO(v0.2): Purity Fix — `std::fs` and `std::io` are heavy filesystem I/O in a
// "graph" crate.  Extract bisect state persistence into a dedicated `arc-bisect-persist`
// boundary crate and keep this crate's graph algorithms pure.
use std::{
    collections::{BTreeMap, BTreeSet, HashSet, VecDeque},
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use arc_algebra_types::Blake3Hash;
use arc_store_types::ChangeId;
use serde::{Deserialize, Serialize};

use crate::graph::ChangeGraph;

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
        self.marks.values().filter(|mark| matches!(mark, BisectMark::Untested)).count()
    }

    /// Count revisions marked [`BisectMark::Good`].
    pub fn good_count(&self) -> usize {
        self.marks.values().filter(|mark| matches!(mark, BisectMark::Good)).count()
    }

    /// Count revisions marked [`BisectMark::Bad`].
    pub fn bad_count(&self) -> usize {
        self.marks.values().filter(|mark| matches!(mark, BisectMark::Bad)).count()
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
    /// Create a new bisect state over a candidate frontier.
    ///
    /// The returned state starts with all candidates marked untested and no
    /// active current revision.
    pub fn start(
        range_expr: String,
        candidates: BTreeSet<ChangeId>,
        find_good: bool,
    ) -> BisectState {
        let mut marks = BTreeMap::new();
        for id in &candidates {
            marks.insert(*id, BisectMark::Untested);
        }
        let started_at_unix =
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        BisectState { range_expr, find_good, candidates, marks, current: None, started_at_unix }
    }

    /// Pick the next candidate using deterministic topological midpoint.
    ///
    /// Determinism guarantee: for a fixed graph and mark-state, this always
    /// returns the same next `ChangeId` across machines and process runs.
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
        if untested.is_empty() { None } else { Some(untested[untested.len() / 2]) }
    }

    /// Apply a test mark and propagate monotonic constraints over the DAG.
    ///
    /// - Marking a node `Good` marks all of its ancestors `Good`.
    /// - Marking a node `Bad` marks all of its descendants `Bad`.
    ///
    /// Returns an error when the new mark contradicts previously committed
    /// evidence.
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
    let parent =
        path.parent().ok_or_else(|| anyhow::anyhow!("path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent)?;

    let tmp_name = format!(
        ".{}.tmp-{}-{}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("bisect"),
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos()
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
                format!("failed to remove stale staged backup {}", staged_backup_path.display())
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
                format!("failed to atomically rename {} -> {}", tmp_path.display(), path.display())
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
            format!("failed to atomically rename {} -> {}", tmp_path.display(), path.display())
        })?;
    }

    if let Ok(dir) = File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use arc_algebra_types::Atom;
    use arc_change::Change;
    use arc_store_types::author;

    use super::*;

    fn make_change(deps: HashSet<Blake3Hash>, label: &str) -> Change {
        let (author, signing_key) = author::test_keypair();
        let content_hash: [u8; 32] = *blake3::hash(label.as_bytes()).as_bytes();
        Change::new(
            deps,
            vec![Atom::Insert { at: vec![label.to_string()], content_hash }],
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
        let state =
            BisectEngine::start("ancestors(@)".to_string(), BTreeSet::from([a, b, c]), false);
        assert_eq!(BisectEngine::select_next(&g, &state), Some(b));
    }

    #[test]
    fn mark_good_propagates_to_ancestors() {
        let (g, a, b, c) = small_chain();
        let mut state =
            BisectEngine::start("ancestors(@)".to_string(), BTreeSet::from([a, b, c]), false);
        BisectEngine::mark(&g, &mut state, c, BisectMark::Good).unwrap();
        assert!(matches!(state.marks.get(&a), Some(BisectMark::Good)));
        assert!(matches!(state.marks.get(&b), Some(BisectMark::Good)));
        assert!(matches!(state.marks.get(&c), Some(BisectMark::Good)));
    }

    #[test]
    fn mark_bad_propagates_to_descendants() {
        let (g, a, b, c) = small_chain();
        let mut state =
            BisectEngine::start("ancestors(@)".to_string(), BTreeSet::from([a, b, c]), false);
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
        let mut state =
            BisectEngine::start("ancestors(@)".to_string(), BTreeSet::from([a, b, c]), false);
        state.current = Some(b);
        save_state(shared, &state).unwrap();
        let loaded = load_state(shared).unwrap().unwrap();
        assert_eq!(loaded.current, Some(b));
        assert_eq!(loaded.candidates, state.candidates);
    }

    #[test]
    fn untested_good_bad_counts() {
        let (_, a, b, c) = small_chain();
        let mut state =
            BisectEngine::start("ancestors(@)".to_string(), BTreeSet::from([a, b, c]), false);
        assert_eq!(state.untested_count(), 3);
        assert_eq!(state.good_count(), 0);
        assert_eq!(state.bad_count(), 0);
        state.marks.insert(a, BisectMark::Good);
        state.marks.insert(b, BisectMark::Bad);
        assert_eq!(state.untested_count(), 1);
        assert_eq!(state.good_count(), 1);
        assert_eq!(state.bad_count(), 1);
    }

    #[test]
    fn select_next_returns_none_when_all_marked() {
        let (g, a, b, c) = small_chain();
        let mut state =
            BisectEngine::start("ancestors(@)".to_string(), BTreeSet::from([a, b, c]), false);
        state.marks.insert(a, BisectMark::Good);
        state.marks.insert(b, BisectMark::Good);
        state.marks.insert(c, BisectMark::Bad);
        assert_eq!(BisectEngine::select_next(&g, &state), None);
    }

    #[test]
    fn mark_untested_resets_mark() {
        let (g, a, b, c) = small_chain();
        let mut state =
            BisectEngine::start("ancestors(@)".to_string(), BTreeSet::from([a, b, c]), false);
        BisectEngine::mark(&g, &mut state, c, BisectMark::Good).unwrap();
        assert_eq!(state.good_count(), 3);
        BisectEngine::mark(&g, &mut state, c, BisectMark::Untested).unwrap();
        assert!(matches!(state.marks.get(&c), Some(BisectMark::Untested)));
        assert!(matches!(state.marks.get(&b), Some(BisectMark::Good)));
        assert!(matches!(state.marks.get(&a), Some(BisectMark::Good)));
    }

    #[test]
    fn mark_good_then_bad_errors() {
        let (g, a, b, c) = small_chain();
        let mut state =
            BisectEngine::start("ancestors(@)".to_string(), BTreeSet::from([a, b, c]), false);
        BisectEngine::mark(&g, &mut state, a, BisectMark::Good).unwrap();
        let result = BisectEngine::mark(&g, &mut state, a, BisectMark::Bad);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already marked good"));
    }

    #[test]
    fn mark_bad_then_good_errors() {
        let (g, a, b, c) = small_chain();
        let mut state =
            BisectEngine::start("ancestors(@)".to_string(), BTreeSet::from([a, b, c]), false);
        BisectEngine::mark(&g, &mut state, a, BisectMark::Bad).unwrap();
        let result = BisectEngine::mark(&g, &mut state, a, BisectMark::Good);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already marked bad"));
    }

    #[test]
    fn load_state_returns_none_when_reset_marker_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let shared = tmp.path();
        let (_, a, b, c) = small_chain();
        let state =
            BisectEngine::start("ancestors(@)".to_string(), BTreeSet::from([a, b, c]), false);
        save_state(shared, &state).unwrap();
        std::fs::write(reset_marker_path(shared), b"reset").unwrap();
        assert!(load_state(shared).unwrap().is_none());
    }

    #[test]
    fn load_state_returns_none_when_no_files() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(load_state(tmp.path()).unwrap().is_none());
    }

    #[test]
    fn load_state_from_backup() {
        let tmp = tempfile::tempdir().unwrap();
        let shared = tmp.path();
        let (_, a, b, c) = small_chain();
        let state =
            BisectEngine::start("ancestors(@)".to_string(), BTreeSet::from([a, b, c]), true);
        let primary = state_path(shared);
        fs::create_dir_all(primary.parent().unwrap()).unwrap();
        let bytes = bincode::serialize(&state).unwrap();
        fs::write(&primary, &bytes).unwrap();
        let backup = primary.with_extension("bak");
        fs::rename(&primary, &backup).unwrap();
        let loaded = load_state(shared).unwrap().unwrap();
        assert_eq!(loaded.candidates, state.candidates);
        assert!(loaded.find_good);
    }

    #[test]
    fn load_state_from_staged_backup() {
        let tmp = tempfile::tempdir().unwrap();
        let shared = tmp.path();
        let (_, a, b, c) = small_chain();
        let state =
            BisectEngine::start("ancestors(@)".to_string(), BTreeSet::from([a, b, c]), false);
        let primary = state_path(shared);
        fs::create_dir_all(primary.parent().unwrap()).unwrap();
        let bytes = bincode::serialize(&state).unwrap();
        let staged = primary.with_extension("bak.new");
        fs::write(&staged, &bytes).unwrap();
        let loaded = load_state(shared).unwrap().unwrap();
        assert_eq!(loaded.candidates, state.candidates);
    }

    #[test]
    fn clear_state_removes_all_files() {
        let tmp = tempfile::tempdir().unwrap();
        let shared = tmp.path();
        let (_, a, b, c) = small_chain();
        let state =
            BisectEngine::start("ancestors(@)".to_string(), BTreeSet::from([a, b, c]), false);
        save_state(shared, &state).unwrap();
        assert!(state_path(shared).exists());
        clear_state(shared).unwrap();
        assert!(!state_path(shared).exists());
        assert!(reset_marker_path(shared).exists());
        assert!(load_state(shared).unwrap().is_none());
    }

    #[test]
    fn save_state_removes_existing_reset_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let shared = tmp.path();
        let (_, a, b, c) = small_chain();
        let state =
            BisectEngine::start("ancestors(@)".to_string(), BTreeSet::from([a, b, c]), false);
        fs::create_dir_all(reset_marker_path(shared).parent().unwrap()).unwrap();
        fs::write(reset_marker_path(shared), b"reset").unwrap();
        assert!(reset_marker_path(shared).exists());
        save_state(shared, &state).unwrap();
        assert!(!reset_marker_path(shared).exists());
    }

    #[test]
    fn state_path_is_correct() {
        let path = state_path(Path::new("/repo"));
        assert_eq!(path, PathBuf::from("/repo/.arc/bisect/state.bin"));
    }

    #[test]
    fn select_next_picks_midpoint_of_four() {
        let mut g = ChangeGraph::new();
        let a = make_change(HashSet::new(), "a");
        let b = make_change(HashSet::from([a.id]), "b");
        let c = make_change(HashSet::from([b.id]), "c");
        let d = make_change(HashSet::from([c.id]), "d");
        g.add_change(a.clone());
        g.add_change(b.clone());
        g.add_change(c.clone());
        g.add_change(d.clone());
        let (aid, bid, cid, did) = (
            ChangeId::from(a.id),
            ChangeId::from(b.id),
            ChangeId::from(c.id),
            ChangeId::from(d.id),
        );
        let state = BisectEngine::start(
            "ancestors(@)".to_string(),
            BTreeSet::from([aid, bid, cid, did]),
            false,
        );
        let next = BisectEngine::select_next(&g, &state).unwrap();
        let ordered = g.topological_sort_ids(&state.candidates);
        let untested: Vec<_> = ordered
            .into_iter()
            .filter(|id| state.marks.get(id) == Some(&BisectMark::Untested))
            .collect();
        assert_eq!(next, untested[untested.len() / 2]);
    }
}
