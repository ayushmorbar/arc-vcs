use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::store::newtypes::{ChangeId, MutationId, SnapshotId};

/// Maximum number of optimistic publish retries before returning an error.
pub const MAX_RETRY_ATTEMPTS: usize = 16;
const STALE_LOCK_TTL_MILLIS: u128 = 30_000;

/// Human or AI actor attribution for an operation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationAgent {
    /// Operation initiated by a human actor.
    #[default]
    Human,
    /// Operation initiated by an AI actor.
    Ai,
}

impl OperationAgent {
    /// Stable user-facing label for CLI output.
    pub fn label(&self) -> &'static str {
        match self {
            OperationAgent::Human => "Human",
            OperationAgent::Ai => "AI",
        }
    }
}

/// Operation category for typed audit filtering.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    /// Ordinary repository mutation.
    #[default]
    Generic,
    /// Rewrite transaction (squash, reorder, amend, diffedit).
    Rewrite,
}

/// Atomic rewrite transaction payload stored as one OpLog node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RewriteTransaction {
    /// Strongly-typed transaction id.
    pub tx_id: MutationId,
    /// User command label (`squash`, `reorder`, ...).
    pub command: String,
    /// Target view name.
    pub view: String,
    /// Heads before rewrite.
    pub before_heads: BTreeSet<ChangeId>,
    /// Heads after rewrite.
    pub after_heads: BTreeSet<ChangeId>,
    /// Old -> new rewritten change map.
    pub rewrite_map: BTreeMap<ChangeId, ChangeId>,
    /// Actor attribution.
    pub agent: OperationAgent,
}

/// User-facing operation metadata recorded in the OpLog.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Operation {
    /// Human-readable short id used by CLI output.
    pub id: String,
    /// Seconds since unix epoch.
    pub timestamp: u64,
    /// Command name that produced this mutation.
    pub command: String,
    /// Stable typed operation kind.
    #[serde(default)]
    pub kind: OperationKind,
    /// View name this operation targeted.
    pub view: String,
    /// Actor attribution for auditing.
    #[serde(default)]
    pub agent: OperationAgent,
    /// View heads before applying the mutation.
    #[serde(alias = "previous_heads")]
    pub before_heads: BTreeSet<ChangeId>,
    /// View heads after applying the mutation.
    #[serde(default)]
    pub after_heads: BTreeSet<ChangeId>,
    /// Rewrite transaction id when `kind == rewrite`.
    #[serde(default)]
    pub tx_id: Option<MutationId>,
    /// Old -> new rewritten id map for rewrite operations.
    #[serde(default)]
    pub rewrite_map: BTreeMap<ChangeId, ChangeId>,
    /// Operation-parent pointers in the OpLog DAG.
    #[serde(default)]
    pub parents: BTreeSet<SnapshotId>,
    /// Deterministic content id for this operation node.
    #[serde(default)]
    pub snapshot: Option<SnapshotId>,
}

impl Operation {
    /// Build a new human-authored operation with current timestamp.
    pub fn new(
        command: impl Into<String>,
        view: impl Into<String>,
        before_heads: BTreeSet<ChangeId>,
        after_heads: BTreeSet<ChangeId>,
    ) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self::new_with_timestamp(command, view, before_heads, after_heads, now)
    }

    /// Build a new operation with explicit actor attribution.
    pub fn new_with_agent(
        command: impl Into<String>,
        view: impl Into<String>,
        before_heads: BTreeSet<ChangeId>,
        after_heads: BTreeSet<ChangeId>,
        agent: OperationAgent,
    ) -> Self {
        let mut op = Self::new(command, view, before_heads, after_heads);
        op.agent = agent;
        op
    }

    fn new_with_timestamp(
        command: impl Into<String>,
        view: impl Into<String>,
        before_heads: BTreeSet<ChangeId>,
        after_heads: BTreeSet<ChangeId>,
        timestamp: u64,
    ) -> Self {
        let command = command.into();
        let view = view.into();
        let mut hasher = blake3::Hasher::new();
        hasher.update(&timestamp.to_le_bytes());
        hasher.update(command.as_bytes());
        hasher.update(view.as_bytes());
        let short = hasher.finalize().to_hex().to_string();
        Self {
            id: short[..8].to_owned(),
            timestamp,
            command,
            kind: OperationKind::Generic,
            view,
            agent: OperationAgent::Human,
            before_heads,
            after_heads,
            tx_id: None,
            rewrite_map: BTreeMap::new(),
            parents: BTreeSet::new(),
            snapshot: None,
        }
    }

    /// Render timestamp as `YYYY-MM-DD HH:MM:SS` in UTC.
    pub fn formatted_time(&self) -> String {
        let mut secs = self.timestamp;
        let second = secs % 60;
        secs /= 60;
        let minute = secs % 60;
        secs /= 60;
        let hour = secs % 24;
        secs /= 24;
        let (year, month, day) = days_to_ymd(secs);
        format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}")
    }

    /// Return the first `before_heads` id as an 8-char hex prefix.
    pub fn before_short(&self) -> String {
        heads_short(&self.before_heads)
    }

    /// Return the first `after_heads` id as an 8-char hex prefix.
    pub fn after_short(&self) -> String {
        heads_short(&self.after_heads)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OperationNode {
    id: SnapshotId,
    operation: Operation,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct HeadsState {
    epoch: u64,
    heads: BTreeSet<SnapshotId>,
}

/// Optimistic, crash-consistent operation-log engine.
pub struct OpLog {
    root: PathBuf,
}

impl OpLog {
    /// Create an OpLog at `<arc_dir>/oplog`.
    pub fn new(arc_dir: &Path) -> Self {
        Self {
            root: arc_dir.join("oplog"),
        }
    }

    /// Persist one operation node and publish it into the head set.
    pub fn append(&self, operation: &Operation) -> Result<()> {
        self.migrate_legacy_json_if_present()?;
        self.ensure_layout()?;

        for attempt in 0..MAX_RETRY_ATTEMPTS {
            let (heads_state, heads_fingerprint) = self.load_heads_state()?;
            let node = self.build_node(operation, &heads_state.heads)?;
            self.persist_node(&node)?;

            let mut next_heads = heads_state.heads.clone();
            for parent in &node.operation.parents {
                let _ = next_heads.remove(parent);
            }
            next_heads.insert(node.id);

            let candidate = HeadsState {
                epoch: heads_state.epoch + 1,
                heads: next_heads,
            };

            if self.publish_heads_cas(&heads_fingerprint, &candidate)? {
                let _ = self.write_legacy_projection();
                return Ok(());
            }

            let jitter_ms = ((attempt as u64) + 1).min(8);
            thread::sleep(Duration::from_millis(jitter_ms));
        }

        anyhow::bail!(
            "failed to append operation after {MAX_RETRY_ATTEMPTS} optimistic retries"
        )
    }

    /// Persist one atomic rewrite transaction operation node.
    pub fn append_transaction(&self, tx: &RewriteTransaction) -> Result<()> {
        let mut op = Operation::new_with_agent(
            tx.command.clone(),
            tx.view.clone(),
            tx.before_heads.clone(),
            tx.after_heads.clone(),
            tx.agent.clone(),
        );
        op.kind = OperationKind::Rewrite;
        op.tx_id = Some(tx.tx_id);
        op.rewrite_map = tx.rewrite_map.clone();
        self.append(&op)
    }

    /// Load all reachable operations from current OpLog heads.
    pub fn read_all(&self) -> Result<Vec<Operation>> {
        self.migrate_legacy_json_if_present()?;
        if !self.heads_file().exists() {
            return Ok(Vec::new());
        }

        let (heads_state, _) = self.load_heads_state()?;
        if heads_state.heads.is_empty() {
            return Ok(Vec::new());
        }

        let mut by_id: HashMap<SnapshotId, OperationNode> = HashMap::new();
        let mut stack: Vec<SnapshotId> = heads_state.heads.iter().copied().collect();

        while let Some(id) = stack.pop() {
            if by_id.contains_key(&id) {
                continue;
            }
            let node = self.load_node(id)?;
            for parent in &node.operation.parents {
                stack.push(*parent);
            }
            by_id.insert(id, node);
        }

        let mut nodes: Vec<OperationNode> = by_id.into_values().collect();
        nodes.sort_by(|a, b| {
            a.operation
                .timestamp
                .cmp(&b.operation.timestamp)
                .then_with(|| a.operation.id.cmp(&b.operation.id))
        });
        Ok(nodes.into_iter().map(|n| n.operation).collect())
    }

    /// Load all reachable operations in reverse chronological order.
    pub fn read_reversed(&self) -> Result<Vec<Operation>> {
        let mut all = self.read_all()?;
        all.reverse();
        Ok(all)
    }

    /// Rewind one published operation from heads and return it.
    pub fn pop(&self) -> Result<Option<Operation>> {
        self.migrate_legacy_json_if_present()?;
        self.ensure_layout()?;
        let lock = PublishLock::acquire(&self.publish_lock_file())?;

        let (heads_state, _) = self.load_heads_state()?;
        if heads_state.heads.is_empty() {
            drop(lock);
            return Ok(None);
        }

        let mut head_nodes: Vec<OperationNode> = heads_state
            .heads
            .iter()
            .copied()
            .map(|id| self.load_node(id))
            .collect::<Result<Vec<_>>>()?;
        head_nodes.sort_by(|a, b| {
            a.operation
                .timestamp
                .cmp(&b.operation.timestamp)
                .then_with(|| a.operation.id.cmp(&b.operation.id))
        });

        let selected = head_nodes
            .pop()
            .ok_or_else(|| anyhow::anyhow!("head set unexpectedly empty"))?;

        let mut next_heads = heads_state.heads.clone();
        let _ = next_heads.remove(&selected.id);
        next_heads.extend(selected.operation.parents.iter().copied());

        let candidate = HeadsState {
            epoch: heads_state.epoch + 1,
            heads: next_heads,
        };
        self.write_heads_state(&candidate)?;
        let _ = self.write_legacy_projection();

        drop(lock);
        Ok(Some(selected.operation))
    }

    fn ensure_layout(&self) -> Result<()> {
        fs::create_dir_all(self.ops_root())?;
        fs::create_dir_all(self.root.join("tmp"))?;
        Ok(())
    }

    fn migrate_legacy_json_if_present(&self) -> Result<()> {
        let Some(legacy_path) = self.legacy_json_path() else {
            return Ok(());
        };

        if self.heads_file().exists() || !legacy_path.exists() {
            return Ok(());
        }

        let json = fs::read_to_string(&legacy_path)
            .with_context(|| format!("failed to read legacy oplog {}", legacy_path.display()))?;
        let legacy_ops: Vec<Operation> =
            serde_json::from_str(&json).context("failed to parse legacy oplog.json")?;

        self.ensure_layout()?;
        let mut heads = BTreeSet::new();
        let mut epoch = 0u64;

        for op in legacy_ops {
            let node = self.build_node(&op, &heads)?;
            self.persist_node(&node)?;
            heads.clear();
            heads.insert(node.id);
            epoch += 1;
        }

        self.write_heads_state(&HeadsState { epoch, heads })?;

        let migrated_path = legacy_path.with_extension("json.migrated");
        if migrated_path.exists() {
            fs::remove_file(&migrated_path).with_context(|| {
                format!(
                    "failed to remove stale legacy migration marker {}",
                    migrated_path.display()
                )
            })?;
        }
        fs::rename(&legacy_path, &migrated_path).with_context(|| {
            format!(
                "failed to rename legacy oplog {} -> {}",
                legacy_path.display(),
                migrated_path.display()
            )
        })?;

        let _ = self.write_legacy_projection();

        Ok(())
    }

    fn write_legacy_projection(&self) -> Result<()> {
        let Some(legacy_path) = self.legacy_json_path() else {
            return Ok(());
        };
        let all = self.read_all()?;
        let json = serde_json::to_vec_pretty(&all).context("failed to serialize legacy oplog")?;
        fs::write(&legacy_path, json)
            .with_context(|| format!("failed to write legacy oplog {}", legacy_path.display()))
    }

    fn build_node(&self, operation: &Operation, current_heads: &BTreeSet<SnapshotId>) -> Result<OperationNode> {
        let mut op = operation.clone();
        op.parents = current_heads.clone();

        let payload = bincode::serialize(&(
            op.timestamp,
            &op.command,
            &op.kind,
            &op.view,
            &op.agent,
            &op.before_heads,
            &op.after_heads,
            &op.tx_id,
            &op.rewrite_map,
            &op.parents,
        ))
        .context("failed to serialize operation payload")?;
        let snapshot = SnapshotId(*blake3::hash(&payload).as_bytes());
        op.snapshot = Some(snapshot);

        Ok(OperationNode {
            id: snapshot,
            operation: op,
        })
    }

    fn persist_node(&self, node: &OperationNode) -> Result<()> {
        let path = self.node_path(node.id);
        if path.exists() {
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes = bincode::serialize(node).context("failed to serialize operation node")?;
        atomic_write_bytes(&path, &bytes)
    }

    fn load_node(&self, id: SnapshotId) -> Result<OperationNode> {
        let path = self.node_path(id);
        let bytes = fs::read(&path)
            .with_context(|| format!("failed to read operation node {}", id.to_hex()))?;
        bincode::deserialize(&bytes)
            .with_context(|| format!("failed to decode operation node {}", id.to_hex()))
    }

    fn load_heads_state(&self) -> Result<(HeadsState, [u8; 32])> {
        let path = self.heads_file();
        let backup = self.heads_backup_file();
        let staged_backup = self.heads_staged_backup_file();
        if !path.exists() {
            if staged_backup.exists() {
                let bytes = fs::read(&staged_backup)
                    .context("failed to read staged backup heads state")?;
                let state: HeadsState = bincode::deserialize(&bytes)
                    .context("failed to deserialize staged backup heads state")?;
                return Ok((state, *blake3::hash(&bytes).as_bytes()));
            }
            if backup.exists() {
                let bytes = fs::read(&backup).context("failed to read backup heads state")?;
                let state: HeadsState = bincode::deserialize(&bytes)
                    .context("failed to deserialize backup heads state")?;
                return Ok((state, *blake3::hash(&bytes).as_bytes()));
            }
            let state = HeadsState::default();
            let bytes = bincode::serialize(&state).context("failed to serialize heads state")?;
            return Ok((state, *blake3::hash(&bytes).as_bytes()));
        }

        let bytes = fs::read(&path).context("failed to read heads state")?;
        let state: HeadsState =
            bincode::deserialize(&bytes).context("failed to deserialize heads state")?;
        Ok((state, *blake3::hash(&bytes).as_bytes()))
    }

    fn publish_heads_cas(&self, expected: &[u8; 32], candidate: &HeadsState) -> Result<bool> {
        let lock = PublishLock::acquire(&self.publish_lock_file())?;

        let (_, current_fingerprint) = self.load_heads_state()?;
        if &current_fingerprint != expected {
            drop(lock);
            return Ok(false);
        }

        self.write_heads_state(candidate)?;
        drop(lock);
        Ok(true)
    }

    fn write_heads_state(&self, state: &HeadsState) -> Result<()> {
        let bytes = bincode::serialize(state).context("failed to serialize heads state")?;
        atomic_write_bytes(&self.heads_file(), &bytes)
    }

    fn ops_root(&self) -> PathBuf {
        self.root.join("ops")
    }

    fn heads_file(&self) -> PathBuf {
        self.root.join("heads.bin")
    }

    fn heads_backup_file(&self) -> PathBuf {
        self.root.join("heads.bin.bak")
    }

    fn heads_staged_backup_file(&self) -> PathBuf {
        self.root.join("heads.bin.bak.new")
    }

    fn publish_lock_file(&self) -> PathBuf {
        self.root.join("publish.lock")
    }

    fn legacy_json_path(&self) -> Option<PathBuf> {
        self.root.parent().map(|arc_dir| arc_dir.join("oplog.json"))
    }

    fn node_path(&self, id: SnapshotId) -> PathBuf {
        let hex = id.to_hex();
        self.ops_root()
            .join(&hex[..2])
            .join(format!("{}.bin", &hex[2..]))
    }
}

struct PublishLock {
    path: PathBuf,
}

impl PublishLock {
    fn acquire(path: &Path) -> Result<Self> {
        let mut retries = 0usize;
        loop {
            match OpenOptions::new().write(true).create_new(true).open(path) {
                Ok(mut file) => {
                    let pid = std::process::id();
                    let _ = writeln!(file, "{pid} {}", now_millis());
                    file.sync_all().ok();
                    return Ok(Self {
                        path: path.to_path_buf(),
                    });
                }
                Err(err) if err.kind() == ErrorKind::AlreadyExists => {
                    if is_stale_lock(path)? {
                        let _ = fs::remove_file(path);
                        continue;
                    }
                    retries += 1;
                    if retries > MAX_RETRY_ATTEMPTS * 8 {
                        anyhow::bail!("timed out acquiring oplog publish lock");
                    }
                    thread::sleep(Duration::from_millis(2));
                }
                Err(err) => return Err(err).context("failed to acquire publish lock"),
            }
        }
    }
}

impl Drop for PublishLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn atomic_write_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent)?;

    let tmp_name = format!(
        ".{}.tmp-{}-{}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("op"),
        std::process::id(),
        now_nanos()
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
        let backup_path = parent.join(format!(
            "{}.bak",
            path.file_name().and_then(|n| n.to_str()).unwrap_or("target")
        ));
        let staged_backup_path = parent.join(format!(
            "{}.bak.new",
            path.file_name().and_then(|n| n.to_str()).unwrap_or("target")
        ));

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
    fs::rename(&tmp_path, path).with_context(|| {
        format!(
            "failed to atomically rename {} -> {}",
            tmp_path.display(),
            path.display()
        )
    })?;

    sync_directory(parent)?;
    Ok(())
}

fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()?;
        Ok(())
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x02000000;
        let open_result = OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(path);
        match open_result {
            Ok(file) => {
                if let Err(err) = file.sync_all()
                    && err.kind() != ErrorKind::PermissionDenied
                {
                    return Err(err.into());
                }
            }
            Err(err) if err.kind() == ErrorKind::PermissionDenied => {
                // Best effort on Windows environments where directory sync is
                // restricted by filesystem policy.
            }
            Err(err) => return Err(err.into()),
        }
        Ok(())
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Ok(())
    }
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn is_stale_lock(path: &Path) -> Result<bool> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err.into()),
    };
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    let Some(ts) = contents.split_whitespace().nth(1) else {
        return Ok(true);
    };
    let Ok(locked_at) = ts.parse::<u128>() else {
        return Ok(true);
    };
    Ok(now_millis().saturating_sub(locked_at) > STALE_LOCK_TTL_MILLIS)
}

fn heads_short(heads: &BTreeSet<ChangeId>) -> String {
    heads
        .iter()
        .next()
        .map(|id| id.to_hex()[..8].to_string())
        .unwrap_or_else(|| "(empty)".to_string())
}

fn days_to_ymd(mut days: u64) -> (u64, u64, u64) {
    let year400 = days / 146_097;
    days %= 146_097;
    let year100 = (days / 36_524).min(3);
    days -= year100 * 36_524;
    let year4 = days / 1_461;
    days %= 1_461;
    let year1 = (days / 365).min(3);
    days -= year1 * 365;
    let year = year400 * 400 + year100 * 100 + year4 * 4 + year1 + 1970;
    let leap = (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400);
    let dims: [u64; 12] = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 1u64;
    for dim in dims {
        if days < dim {
            break;
        }
        days -= dim;
        month += 1;
    }
    (year, month, days + 1)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};

    use super::*;

    fn cid(byte: u8) -> ChangeId {
        ChangeId([byte; 32])
    }

    #[test]
    fn operation_id_is_deterministic_for_fixed_timestamp() {
        let a = Operation::new_with_timestamp("snap", "main", BTreeSet::new(), BTreeSet::new(), 1234);
        let b = Operation::new_with_timestamp("snap", "main", BTreeSet::new(), BTreeSet::new(), 1234);
        assert_eq!(a.id, b.id);
    }

    #[test]
    fn append_read_pop_roundtrip() {
        let dir = tempfile::tempdir().expect("tempdir must succeed");
        let log = OpLog::new(dir.path());

        let op1 = Operation::new_with_timestamp(
            "snap",
            "main",
            BTreeSet::from([cid(1)]),
            BTreeSet::from([cid(2)]),
            100,
        );
        let op2 = Operation::new_with_timestamp(
            "merge",
            "main",
            BTreeSet::from([cid(2)]),
            BTreeSet::from([cid(3)]),
            101,
        );

        log.append(&op1).expect("append op1 must succeed");
        log.append(&op2).expect("append op2 must succeed");

        let all = log.read_all().expect("read_all must succeed");
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].command, "snap");
        assert_eq!(all[1].command, "merge");

        let popped = log.pop().expect("pop must succeed").expect("pop must return op");
        assert_eq!(popped.command, "merge");

        let remaining = log.read_all().expect("read_all must succeed");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].command, "snap");
    }

    #[test]
    fn concurrent_append_preserves_both_operations() {
        let dir = tempfile::tempdir().expect("tempdir must succeed");
        let log1 = Arc::new(OpLog::new(dir.path()));
        let log2 = Arc::clone(&log1);
        let barrier = Arc::new(Barrier::new(2));

        let b1 = Arc::clone(&barrier);
        let t1 = std::thread::spawn(move || {
            let op = Operation::new("snap", "main", BTreeSet::new(), BTreeSet::from([cid(1)]));
            b1.wait();
            log1.append(&op)
        });

        let b2 = Arc::clone(&barrier);
        let t2 = std::thread::spawn(move || {
            let op = Operation::new("merge", "main", BTreeSet::from([cid(1)]), BTreeSet::from([cid(2)]));
            b2.wait();
            log2.append(&op)
        });

        t1.join().expect("thread 1 must not panic").expect("append1 must succeed");
        t2.join().expect("thread 2 must not panic").expect("append2 must succeed");

        let ops = OpLog::new(dir.path()).read_all().expect("read_all must succeed");
        assert_eq!(ops.len(), 2);
    }

    #[test]
    fn append_transaction_roundtrip() {
        let dir = tempfile::tempdir().expect("tempdir must succeed");
        let log = OpLog::new(dir.path());

        let tx = RewriteTransaction {
            tx_id: MutationId([9u8; 32]),
            command: "reorder".to_string(),
            view: "main".to_string(),
            before_heads: BTreeSet::from([cid(1)]),
            after_heads: BTreeSet::from([cid(2)]),
            rewrite_map: BTreeMap::from([(cid(1), cid(2))]),
            agent: OperationAgent::Human,
        };

        log.append_transaction(&tx)
            .expect("append transaction must succeed");
        let ops = log.read_all().expect("read_all must succeed");
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0].kind, OperationKind::Rewrite));
        assert_eq!(ops[0].tx_id, Some(tx.tx_id));
        assert_eq!(ops[0].rewrite_map, tx.rewrite_map);
    }

    #[test]
    fn legacy_previous_heads_alias_still_deserializes() {
        let json = r#"{
            "id": "abcd1234",
            "timestamp": 1700000000,
            "command": "snap",
            "view": "main",
            "previous_heads": [[1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1]]
        }"#;
        let op: Operation = serde_json::from_str(json).expect("legacy payload must deserialize");
        assert_eq!(op.command, "snap");
        assert_eq!(op.before_heads.len(), 1);
        assert_eq!(op.agent, OperationAgent::Human);
    }

    #[test]
    fn migrates_legacy_json_log() {
        let dir = tempfile::tempdir().expect("tempdir must succeed");
        let arc_dir = dir.path();
        let legacy_path = arc_dir.join("oplog.json");

        let legacy = vec![
            Operation::new_with_timestamp(
                "snap",
                "main",
                BTreeSet::new(),
                BTreeSet::from([cid(1)]),
                100,
            ),
            Operation::new_with_timestamp(
                "merge",
                "main",
                BTreeSet::from([cid(1)]),
                BTreeSet::from([cid(2)]),
                101,
            ),
        ];
        let json = serde_json::to_string_pretty(&legacy).expect("legacy json must serialize");
        fs::write(&legacy_path, json).expect("legacy log write must succeed");

        let log = OpLog::new(arc_dir);
        let all = log.read_all().expect("migration read must succeed");
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].command, "snap");
        assert_eq!(all[1].command, "merge");
        assert!(
            arc_dir.join("oplog.json.migrated").exists(),
            "legacy oplog should be moved aside after migration"
        );
    }
}
