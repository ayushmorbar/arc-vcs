//! BLUF: `arc-git` is the Git ingress edge for the `arc` DAG.
//!
//! It reads legacy Git repositories and emits deterministic commit/tree/blob
//! structures that upstream `arc` crates can translate into Spacetime-DAG
//! changes and CRDT-algebra operations.
//!
//! ## Purity and I/O boundary
//!
//! This crate is an I/O boundary by design:
//! - It performs filesystem reads of `.git` refs, loose objects, and packfiles.
//! - It performs pure parsing/walking after bytes are loaded.
//! - It does not mutate repository state.
//!
//! ## Why this crate exists
//!
//! The `arc` architecture keeps Git compatibility concerns outside algebra and
//! provenance layers. `arc-git` isolates SHA-1 object decoding and history walk
//! logic so Ed25519 provenance and CRDT semantics remain independent from Git
//! storage internals.
//!
//! ## Example
//!
//! ```no_run
//! use std::path::Path;
//!
//! let analysis = arc_git::analyze_git_repo(Path::new("."))?;
//! println!("HEAD={} commits={}", analysis.head_hex, analysis.commit_count);
//! # Ok::<(), anyhow::Error>(())
//! ```

use anyhow::{Context, Result, bail};
use bytes::Bytes;
use memmap2::{Mmap, MmapOptions};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

// -- types --------------------------------------------------------------------

/// A 20-byte SHA-1 object identifier - Git's native hash format.
pub type GitOid = [u8; 20];

/// Git object type tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObjKind {
    Commit,
    Tree,
    Blob,
    Tag,
}

/// Decoded Git object: kind + raw payload (header already stripped).
///
/// This type is intentionally crate-private so raw Git storage bytes are
/// translated into `GitCommit`/`GitTree` domain structures before crossing
/// the arc-git boundary.
struct RawObject {
    kind: ObjKind,
    data: Bytes,
}

/// Parsed metadata extracted from a single Git commit object.
#[derive(Debug, Clone)]
pub struct GitCommit {
    /// SHA-1 hash of this commit.
    pub oid: GitOid,
    /// SHA-1 of the root tree object.
    pub tree: GitOid,
    /// Parent commit OIDs (empty for root commits).
    pub parents: Vec<GitOid>,
    /// Author name.
    pub author_name: String,
    /// Author email.
    pub author_email: String,
    /// Author-date as a Unix timestamp (seconds since epoch).
    pub author_timestamp: i64,
    /// Committer name.
    pub committer_name: String,
    /// Committer email.
    pub committer_email: String,
    /// Full commit message (subject + body).
    pub message: String,
}

/// Summary returned after analysing a legacy Git repository.
#[derive(Debug)]
pub struct GitAnalysis {
    /// Filesystem path that was analysed.
    pub path: PathBuf,
    /// HEAD commit as a 40-char lowercase hex string.
    pub head_hex: String,
    /// Total number of reachable commits.
    pub commit_count: usize,
    /// All reachable commits in topological order, **oldest first**.
    pub commits: Vec<GitCommit>,
}

const TEST_TRAVERSAL_AUTO: u8 = 0;
const TEST_TRAVERSAL_COMMIT_GRAPH_ONLY: u8 = 1;
const TEST_TRAVERSAL_LEGACY_ONLY: u8 = 2;
const TEST_TRAVERSAL_FORCE_GRAPH_FAIL: u8 = 3;

#[cfg(test)]
thread_local! {
    static TEST_TRAVERSAL_OVERRIDE: std::cell::Cell<u8> = const { std::cell::Cell::new(TEST_TRAVERSAL_AUTO) };
}

#[cfg(test)]
fn set_test_traversal_override(mode: u8) -> u8 {
    TEST_TRAVERSAL_OVERRIDE.with(|cell| {
        let prev = cell.get();
        cell.set(mode);
        prev
    })
}

#[cfg(test)]
fn get_test_traversal_override() -> u8 {
    TEST_TRAVERSAL_OVERRIDE.with(std::cell::Cell::get)
}

// -- public API ---------------------------------------------------------------

/// Open the Git repository at `path`, walk every reachable commit from
/// HEAD, and return structured analysis with commits **oldest-first**.
pub fn analyze_git_repo(path: &Path) -> Result<GitAnalysis> {
    let git_dir = resolve_git_dir(path)?;
    let head_oid = resolve_head(&git_dir)?;
    let head_hex = oid_hex(&head_oid);

    let commits = collect_commits_with_fallback(&git_dir, head_oid)?;

    Ok(GitAnalysis {
        path: path.to_path_buf(),
        head_hex,
        commit_count: commits.len(),
        commits,
    })
}

fn collect_commits_with_fallback(git_dir: &Path, head_oid: GitOid) -> Result<Vec<GitCommit>> {
    #[cfg(test)]
    let mode = get_test_traversal_override();
    #[cfg(not(test))]
    let mode = TEST_TRAVERSAL_AUTO;

    match mode {
        TEST_TRAVERSAL_LEGACY_ONLY => collect_commits_legacy(git_dir, head_oid),
        TEST_TRAVERSAL_COMMIT_GRAPH_ONLY => collect_commits_commit_graph(git_dir, head_oid),
        TEST_TRAVERSAL_FORCE_GRAPH_FAIL => collect_commits_commit_graph(git_dir, head_oid)
            .or_else(|_| collect_commits_legacy(git_dir, head_oid)),
        _ => collect_commits_commit_graph(git_dir, head_oid)
            .or_else(|_| collect_commits_legacy(git_dir, head_oid)),
    }
}

fn collect_commits_legacy(git_dir: &Path, head_oid: GitOid) -> Result<Vec<GitCommit>> {
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    let mut commits = Vec::new();

    queue.push_back(head_oid);
    while let Some(oid) = queue.pop_front() {
        if !visited.insert(oid) {
            continue;
        }
        let obj = read_object(git_dir, &oid)?;
        if obj.kind != ObjKind::Commit {
            continue;
        }
        let commit = parse_commit(&oid, &obj.data)?;
        for p in &commit.parents {
            queue.push_back(*p);
        }
        commits.push(commit);
    }

    // Reverse BFS order -> oldest commit first (natural for replaying).
    commits.reverse();

    Ok(commits)
}

fn collect_commits_commit_graph(git_dir: &Path, head_oid: GitOid) -> Result<Vec<GitCommit>> {
    #[cfg(test)]
    if get_test_traversal_override() == TEST_TRAVERSAL_FORCE_GRAPH_FAIL {
        bail!("forced commit-graph traversal failure");
    }

    let graph = CommitGraphIndex::open(git_dir)?;
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    let mut order = Vec::new();

    queue.push_back(head_oid);
    while let Some(oid) = queue.pop_front() {
        if !visited.insert(oid) {
            continue;
        }
        let parents = graph.parent_oids_for(&oid)?;
        for parent in parents {
            queue.push_back(parent);
        }
        order.push(oid);
    }

    order.reverse();
    let mut commits = Vec::with_capacity(order.len());
    for oid in order {
        let obj = read_object(git_dir, &oid)?;
        if obj.kind != ObjKind::Commit {
            continue;
        }
        // Domain boundary: commit-graph traversal emits Git OIDs only; full commit
        // domain values are materialized through commit parsing at arc-git boundary.
        commits.push(parse_commit(&oid, &obj.data)?);
    }

    Ok(commits)
}

/// Render a [`GitOid`] as a 40-char lowercase hex string.
pub fn oid_hex(oid: &GitOid) -> String {
    oid.iter().map(|b| format!("{b:02x}")).collect()
}

// -- git dir resolution -------------------------------------------------------

/// Locate the `.git` directory for the repository rooted at `path`.
///
/// Handles plain repositories (`.git/` subdirectory), worktrees
/// (`.git` file with a `gitdir:` pointer), and bare repositories.
pub fn resolve_git_dir(path: &Path) -> Result<PathBuf> {
    let dot_git = path.join(".git");
    if dot_git.is_dir() {
        return Ok(dot_git);
    }
    // `.git` can be a file in worktrees / submodules: "gitdir: <path>"
    if dot_git.is_file() {
        let content = std::fs::read_to_string(&dot_git)?;
        if let Some(target) = content.trim().strip_prefix("gitdir: ") {
            let resolved = if Path::new(target).is_absolute() {
                PathBuf::from(target)
            } else {
                path.join(target)
            };
            return Ok(resolved);
        }
    }
    // Bare repository.
    if path.join("HEAD").exists() && path.join("objects").is_dir() {
        return Ok(path.to_path_buf());
    }
    bail!("no Git repository at '{}'", path.display());
}

fn resolve_head(git_dir: &Path) -> Result<GitOid> {
    let raw = std::fs::read_to_string(git_dir.join("HEAD")).context("failed to read .git/HEAD")?;
    let raw = raw.trim();
    if let Some(refpath) = raw.strip_prefix("ref: ") {
        resolve_ref(git_dir, refpath)
    } else {
        parse_hex_oid(raw)
    }
}

/// Resolve a ref name (e.g. `refs/heads/main`) -> OID.
/// Checks the loose file first, then falls back to `packed-refs`.
fn resolve_ref(git_dir: &Path, refpath: &str) -> Result<GitOid> {
    // Loose ref
    let loose = git_dir.join(refpath);
    if loose.is_file() {
        let hex = std::fs::read_to_string(&loose)?;
        return parse_hex_oid(hex.trim());
    }
    // Packed refs
    let packed = git_dir.join("packed-refs");
    if packed.is_file() {
        for line in std::fs::read_to_string(&packed)?.lines() {
            if line.starts_with('#') || line.starts_with('^') {
                continue;
            }
            let mut parts = line.splitn(2, ' ');
            if let (Some(hex), Some(name)) = (parts.next(), parts.next())
                && name == refpath
            {
                return parse_hex_oid(hex);
            }
        }
    }
    bail!("cannot resolve ref '{refpath}'");
}

// -- object I/O ---------------------------------------------------------------

fn read_object(git_dir: &Path, oid: &GitOid) -> Result<RawObject> {
    read_loose_object(git_dir, oid)
        .or_else(|_| read_packed_object(git_dir, oid))
        .with_context(|| format!("object {} not found", oid_hex(oid)))
}

fn read_loose_object(git_dir: &Path, oid: &GitOid) -> Result<RawObject> {
    let h = oid_hex(oid);
    let path = git_dir.join("objects").join(&h[..2]).join(&h[2..]);
    let compressed = std::fs::read(&path)?;
    let buf = zlib_decompress(&compressed)?;
    let nul = buf
        .iter()
        .position(|&b| b == 0)
        .context("malformed loose object: no NUL separator")?;
    let header = std::str::from_utf8(&buf[..nul])?;
    let kind = parse_obj_kind(header.split(' ').next().unwrap_or(""))?;
    Ok(RawObject {
        kind,
        data: Bytes::copy_from_slice(&buf[nul + 1..]),
    })
}

// -- pack files ---------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
enum PackLookupBackend {
    /// Fast path: memory-map `.idx` and `.pack` files and decode index tables directly.
    ///
    /// This keeps legacy Git ingress zero-copy at the filesystem boundary and minimizes
    /// heap pressure when scanning large repositories.
    MmapChunkIndex,
    /// Compatibility path: pre-existing heap-backed index/pack scanning.
    LegacyIndexScan,
}

const TEST_BACKEND_AUTO: u8 = 0;
const TEST_BACKEND_MMAP_ONLY: u8 = 1;
const TEST_BACKEND_LEGACY_ONLY: u8 = 2;
const TEST_BACKEND_FORCE_MMAP_FAIL: u8 = 3;

#[cfg(test)]
thread_local! {
    static TEST_BACKEND_OVERRIDE: std::cell::Cell<u8> = const { std::cell::Cell::new(TEST_BACKEND_AUTO) };
}

#[cfg(test)]
fn set_test_backend_override(mode: u8) -> u8 {
    TEST_BACKEND_OVERRIDE.with(|cell| {
        let prev = cell.get();
        cell.set(mode);
        prev
    })
}

#[cfg(test)]
fn get_test_backend_override() -> u8 {
    TEST_BACKEND_OVERRIDE.with(std::cell::Cell::get)
}

/// Read an object from pack storage using a tiered backend strategy.
///
/// Why memory mapping for legacy ingress:
/// Git pack/index files are immutable snapshots. Mapping them read-only lets us
/// parse fan-out and offset tables directly from kernel-backed pages without first
/// copying the whole file into process-owned heap buffers. This improves startup
/// latency and peak memory while preserving deterministic parsing behavior.
fn read_packed_object(git_dir: &Path, oid: &GitOid) -> Result<RawObject> {
    #[cfg(test)]
    let mode = get_test_backend_override();
    #[cfg(not(test))]
    let mode = TEST_BACKEND_AUTO;

    let backends: &[PackLookupBackend] = match mode {
        TEST_BACKEND_MMAP_ONLY => &[PackLookupBackend::MmapChunkIndex],
        TEST_BACKEND_LEGACY_ONLY => &[PackLookupBackend::LegacyIndexScan],
        _ => &[
            PackLookupBackend::MmapChunkIndex,
            PackLookupBackend::LegacyIndexScan,
        ],
    };

    let mut errors = Vec::new();

    for backend in backends {
        if mode == TEST_BACKEND_FORCE_MMAP_FAIL
            && matches!(backend, PackLookupBackend::MmapChunkIndex)
        {
            errors.push("mmap backend failed: forced test failure".to_string());
            continue;
        }

        let attempt = match backend {
            PackLookupBackend::MmapChunkIndex => read_packed_object_mmap(git_dir, oid),
            PackLookupBackend::LegacyIndexScan => read_packed_object_legacy(git_dir, oid),
        };
        match attempt {
            Ok(obj) => return Ok(obj),
            Err(err) => {
                let label = match backend {
                    PackLookupBackend::MmapChunkIndex => "mmap backend",
                    PackLookupBackend::LegacyIndexScan => "legacy backend",
                };
                errors.push(format!("{label} failed: {err:#}"));
            }
        }
    }

    if errors.is_empty() {
        bail!("no pack backend available");
    }
    bail!("all pack backends failed:\n{}", errors.join("\n"))
}

fn read_packed_object_mmap(git_dir: &Path, oid: &GitOid) -> Result<RawObject> {
    let pack_dir = git_dir.join("objects").join("pack");
    if !pack_dir.is_dir() {
        bail!("no pack directory");
    }
    for entry in std::fs::read_dir(&pack_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        if !name.to_string_lossy().ends_with(".idx") {
            continue;
        }
        let idx_path = entry.path();
        let pack_path = idx_path.with_extension("pack");
        if !pack_path.exists() {
            continue;
        }

        let idx = mmap_read_only(&idx_path)?;
        let pack = mmap_read_only(&pack_path)?;
        if let Ok(obj) = lookup_in_pack(&idx, &pack, oid, git_dir) {
            return Ok(obj);
        }
    }
    bail!("object not in any pack");
}

fn read_packed_object_legacy(git_dir: &Path, oid: &GitOid) -> Result<RawObject> {
    let pack_dir = git_dir.join("objects").join("pack");
    if !pack_dir.is_dir() {
        bail!("no pack directory");
    }
    for entry in std::fs::read_dir(&pack_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        if !name.to_string_lossy().ends_with(".idx") {
            continue;
        }
        let idx_path = entry.path();
        let pack_path = idx_path.with_extension("pack");
        if !pack_path.exists() {
            continue;
        }
        let idx = std::fs::read(&idx_path)?;
        let pack = std::fs::read(&pack_path)?;
        if let Ok(obj) = lookup_in_pack(&idx, &pack, oid, git_dir) {
            return Ok(obj);
        }
    }
    bail!("object not in any pack");
}

fn mmap_read_only(path: &Path) -> Result<Mmap> {
    let file = File::open(path).with_context(|| format!("failed to open '{}'", path.display()))?;
    // SAFETY: pack and index files are immutable snapshots created atomically by git.
    let map = unsafe { MmapOptions::new().map_copy_read_only(&file) }
        .with_context(|| format!("failed to mmap '{}'", path.display()))?;
    Ok(map)
}

// -- commit-graph traversal --------------------------------------------------

const COMMIT_GRAPH_SIGNATURE: &[u8; 4] = b"CGPH";
const COMMIT_GRAPH_HEADER_LEN: usize = 8;
const COMMIT_GRAPH_FAN_LEN: usize = 256;
const COMMIT_GRAPH_COMMIT_DATA_SANS_HASH: usize = 16;
const COMMIT_GRAPH_NO_PARENT: u32 = 0x7000_0000;
const COMMIT_GRAPH_EXTENDED_EDGE_MASK: u32 = 0x8000_0000;

/// Read-only commit-graph view backed by an mmap.
///
/// The parser validates chunk table offsets and chunk sizes before exposing
/// accessors. Traversal consumes only `GitOid` parent relationships so raw
/// chunk offsets never leave this internal ingress boundary.
struct CommitGraphIndex {
    data: Mmap,
    hash_len: usize,
    fan: [u32; COMMIT_GRAPH_FAN_LEN],
    oid_lookup_offset: usize,
    commit_data_offset: usize,
    edge_chunk_range: Option<std::ops::Range<usize>>,
    commit_count: u32,
}

impl CommitGraphIndex {
    /// Open and validate `.git/objects/info/commit-graph` for accelerated history walk.
    ///
    /// Fallback behavior:
    /// Callers should treat any error from this constructor as a soft failure and
    /// switch to legacy commit object traversal. This keeps ingestion resilient for
    /// repositories without commit-graph support or with malformed/stale metadata.
    fn open(git_dir: &Path) -> Result<Self> {
        let path = resolve_commit_graph_path(git_dir)?;
        let data = mmap_read_only(&path)?;
        Self::from_mmap(data)
    }

    fn from_mmap(data: Mmap) -> Result<Self> {
        if data.len() < COMMIT_GRAPH_HEADER_LEN {
            bail!("commit-graph is too small");
        }

        if data
            .get(0..4)
            .context("commit-graph header missing signature")?
            != COMMIT_GRAPH_SIGNATURE
        {
            bail!("commit-graph signature mismatch");
        }

        let version = *data.get(4).context("commit-graph missing version")?;
        if version != 1 {
            bail!("unsupported commit-graph version {version}");
        }

        let hash_version = *data.get(5).context("commit-graph missing hash version")?;
        if hash_version != 1 {
            bail!("unsupported commit-graph hash version {hash_version}");
        }
        let hash_len = 20usize;

        let chunk_count = *data.get(6).context("commit-graph missing chunk count")? as usize;
        let base_graph_count = *data
            .get(7)
            .context("commit-graph missing base graph count")?;
        if base_graph_count != 0 {
            bail!(
                "split commit-graph chains are not supported in arc-git fast path; fallback required"
            );
        }
        if chunk_count == 0 {
            bail!("commit-graph chunk table is empty");
        }

        let toc_bytes = (chunk_count + 1)
            .checked_mul(12)
            .context("commit-graph table-of-contents size overflow")?;
        let toc_start = COMMIT_GRAPH_HEADER_LEN;
        let toc_end = toc_start
            .checked_add(toc_bytes)
            .context("commit-graph table-of-contents range overflow")?;
        if toc_end > data.len() {
            bail!("commit-graph table-of-contents exceeds file length");
        }

        let mut chunks: HashMap<[u8; 4], std::ops::Range<usize>> = HashMap::new();
        let mut entries = Vec::with_capacity(chunk_count + 1);
        let mut cursor = toc_start;
        for _ in 0..=chunk_count {
            let id: [u8; 4] = data
                .get(cursor..cursor + 4)
                .context("commit-graph truncated chunk id")?
                .try_into()?;
            let offset = usize::try_from(read_u64_be_at(
                &data,
                cursor + 4,
                "commit-graph chunk offset",
            )?)
            .context("commit-graph chunk offset does not fit in usize")?;
            entries.push((id, offset));
            cursor += 12;
        }

        let (sentinel_id, _) = entries.last().context("missing commit-graph sentinel")?;
        if *sentinel_id != [0, 0, 0, 0] {
            bail!("commit-graph chunk sentinel missing");
        }

        for window in entries.windows(2) {
            let (id, start) = window[0];
            let (_, end) = window[1];
            if end <= start {
                bail!("commit-graph chunk offsets are not strictly increasing");
            }
            if start < toc_end {
                bail!("commit-graph chunk overlaps table-of-contents region");
            }
            if end > data.len() {
                bail!("commit-graph chunk exceeds file length");
            }
            if chunks.contains_key(&id) {
                bail!("commit-graph contains duplicate chunk id");
            }
            chunks.insert(id, start..end);
        }

        let oidf = chunks
            .get(b"OIDF")
            .context("commit-graph missing OIDF chunk")?
            .clone();
        let oidl = chunks
            .get(b"OIDL")
            .context("commit-graph missing OIDL chunk")?
            .clone();
        let cdat = chunks
            .get(b"CDAT")
            .context("commit-graph missing CDAT chunk")?
            .clone();
        let edge = chunks.get(b"EDGE").cloned();

        if oidf.len() != COMMIT_GRAPH_FAN_LEN * 4 {
            bail!("commit-graph OIDF chunk has invalid size");
        }

        let mut fan = [0u32; COMMIT_GRAPH_FAN_LEN];
        for (idx, slot) in fan.iter_mut().enumerate() {
            *slot = read_u32_be_at(&data, oidf.start + idx * 4, "commit-graph fanout")?;
        }
        let commit_count = fan[255];
        let commit_count_usize = commit_count as usize;

        let expected_oidl = commit_count_usize
            .checked_mul(hash_len)
            .context("commit-graph OIDL size overflow")?;
        if oidl.len() != expected_oidl {
            bail!("commit-graph OIDL chunk length does not match fanout count");
        }

        let cdat_entry_size = hash_len + COMMIT_GRAPH_COMMIT_DATA_SANS_HASH;
        let expected_cdat = commit_count_usize
            .checked_mul(cdat_entry_size)
            .context("commit-graph CDAT size overflow")?;
        if cdat.len() != expected_cdat {
            bail!("commit-graph CDAT chunk length does not match fanout count");
        }

        if let Some(edge_range) = &edge
            && edge_range.len() % 4 != 0
        {
            bail!("commit-graph EDGE chunk has non-u32 length");
        }

        Ok(Self {
            data,
            hash_len,
            fan,
            oid_lookup_offset: oidl.start,
            commit_data_offset: cdat.start,
            edge_chunk_range: edge,
            commit_count,
        })
    }

    fn lookup_position(&self, oid: &GitOid) -> Option<u32> {
        let first = oid[0] as usize;
        let mut upper = self.fan[first];
        let mut lower = if first == 0 { 0 } else { self.fan[first - 1] };

        while lower < upper {
            let mid = (lower + upper) / 2;
            let mid_oid = self.oid_at(mid).ok()?;
            match oid.as_slice().cmp(mid_oid.as_slice()) {
                std::cmp::Ordering::Less => upper = mid,
                std::cmp::Ordering::Equal => return Some(mid),
                std::cmp::Ordering::Greater => lower = mid + 1,
            }
        }
        None
    }

    fn oid_at(&self, pos: u32) -> Result<GitOid> {
        if pos >= self.commit_count {
            bail!("commit-graph position {pos} out of bounds");
        }
        let start = self
            .oid_lookup_offset
            .checked_add(pos as usize * self.hash_len)
            .context("commit-graph OIDL offset overflow")?;
        let slice = self
            .data
            .get(start..start + self.hash_len)
            .context("commit-graph OIDL entry out of bounds")?;
        slice
            .try_into()
            .map_err(|_| anyhow::anyhow!("commit-graph OIDL entry has invalid hash width"))
    }

    fn parent_oids_for(&self, oid: &GitOid) -> Result<Vec<GitOid>> {
        let pos = self
            .lookup_position(oid)
            .context("HEAD or parent OID is not present in commit-graph")?;
        let parent_positions = self.parent_positions_for(pos)?;
        parent_positions
            .into_iter()
            .map(|p| self.oid_at(p))
            .collect()
    }

    fn parent_positions_for(&self, pos: u32) -> Result<Vec<u32>> {
        if pos >= self.commit_count {
            bail!("commit-graph position {pos} out of bounds");
        }

        let entry_size = self.hash_len + COMMIT_GRAPH_COMMIT_DATA_SANS_HASH;
        let start = self
            .commit_data_offset
            .checked_add(pos as usize * entry_size)
            .context("commit-graph CDAT entry offset overflow")?;
        let entry = self
            .data
            .get(start..start + entry_size)
            .context("commit-graph CDAT entry out of bounds")?;

        let parent1 = read_u32_from(entry, self.hash_len, "commit-graph parent1")?;
        let parent2 = read_u32_from(entry, self.hash_len + 4, "commit-graph parent2")?;

        let mut parents = Vec::new();
        if parent1 != COMMIT_GRAPH_NO_PARENT {
            if parent1 & COMMIT_GRAPH_EXTENDED_EDGE_MASK != 0 {
                bail!("commit-graph parent1 cannot reference EDGE list");
            }
            parents.push(self.validate_parent_position(parent1)?);
        }

        if parent2 == COMMIT_GRAPH_NO_PARENT {
            return Ok(parents);
        }

        if parent2 & COMMIT_GRAPH_EXTENDED_EDGE_MASK == 0 {
            parents.push(self.validate_parent_position(parent2)?);
            return Ok(parents);
        }

        let edge_idx = parent2 & !COMMIT_GRAPH_EXTENDED_EDGE_MASK;
        let edge_range = self
            .edge_chunk_range
            .clone()
            .context("commit-graph parent references missing EDGE chunk")?;
        let mut cursor = edge_range
            .start
            .checked_add(edge_idx as usize * 4)
            .context("commit-graph EDGE offset overflow")?;

        loop {
            let raw = read_u32_be_at(&self.data, cursor, "commit-graph EDGE parent")?;
            cursor = cursor
                .checked_add(4)
                .context("commit-graph EDGE cursor overflow")?;

            if raw & COMMIT_GRAPH_EXTENDED_EDGE_MASK != 0 {
                let last = raw & !COMMIT_GRAPH_EXTENDED_EDGE_MASK;
                parents.push(self.validate_parent_position(last)?);
                break;
            }
            parents.push(self.validate_parent_position(raw)?);

            if cursor > edge_range.end {
                bail!("commit-graph EDGE traversal overflow");
            }
        }

        Ok(parents)
    }

    fn validate_parent_position(&self, pos: u32) -> Result<u32> {
        if pos >= self.commit_count {
            bail!("commit-graph parent position {pos} out of bounds");
        }
        Ok(pos)
    }
}

fn resolve_commit_graph_path(git_dir: &Path) -> Result<PathBuf> {
    let info_dir = git_dir.join("objects").join("info");
    let monolithic = info_dir.join("commit-graph");
    if monolithic.is_file() {
        return Ok(monolithic);
    }

    let split_dir = info_dir.join("commit-graphs");
    let chain = split_dir.join("commit-graph-chain");
    if chain.is_file() {
        let content = std::fs::read_to_string(&chain)
            .with_context(|| format!("failed to read '{}'", chain.display()))?;
        let last = content
            .lines()
            .map(str::trim)
            .rfind(|l| !l.is_empty())
            .context("commit-graph chain file is empty")?;
        return Ok(split_dir.join(format!("graph-{last}.graph")));
    }

    bail!("commit-graph file not found")
}

fn read_u32_from(buf: &[u8], offset: usize, what: &str) -> Result<u32> {
    let bytes = buf
        .get(offset..offset + 4)
        .with_context(|| format!("truncated {what} at offset {offset}"))?;
    let mut arr = [0u8; 4];
    arr.copy_from_slice(bytes);
    Ok(u32::from_be_bytes(arr))
}

/// Look up `oid` in a **v2** pack index, then extract from the `.pack`.
fn lookup_in_pack(idx: &[u8], pack: &[u8], oid: &GitOid, git_dir: &Path) -> Result<RawObject> {
    // -- v2 index header ------------------------------------------------
    //  0..4   magic  0xff 't' 'O' 'c'
    //  4..8   version (2)
    //  8..1032  256-entry fan-out table (4 bytes each, big-endian)
    if idx.len() < 1032 || &idx[..4] != b"\xfftOc" {
        bail!("unsupported pack index format");
    }
    let ver = read_u32_be_at(idx, 4, "pack index version")?;
    if ver != 2 {
        bail!("pack index v{ver} not supported (only v2)");
    }

    let fanout = |i: usize| -> Result<u32> {
        let off = 8 + i * 4;
        read_u32_be_at(idx, off, "pack index fanout")
    };
    let total = fanout(255)? as usize;

    let fb = oid[0] as usize;
    let lo = if fb == 0 { 0 } else { fanout(fb - 1)? as usize };
    let hi = fanout(fb)? as usize;
    if lo > hi || hi > total {
        bail!("invalid fanout range [{lo}, {hi}) for total {total}");
    }

    let oid_table: usize = 1032; // 8 + 256*4
    let crc_table = oid_table
        .checked_add(
            total
                .checked_mul(20)
                .context("pack idx oid table overflow")?,
        )
        .context("pack idx crc table overflow")?;
    let off_table = crc_table
        .checked_add(total.checked_mul(4).context("pack idx crc span overflow")?)
        .context("pack idx offset table overflow")?;
    let big_table = off_table
        .checked_add(
            total
                .checked_mul(4)
                .context("pack idx offset span overflow")?,
        )
        .context("pack idx big-offset table overflow")?;
    if big_table > idx.len() {
        bail!("pack idx tables exceed file length");
    }

    // Binary search in the sorted OID table [lo, hi).
    let idx_pos = {
        let (mut l, mut r) = (lo, hi);
        let mut found = None;
        while l < r {
            let m = l + (r - l) / 2;
            let start = oid_table + m * 20;
            let entry = idx
                .get(start..start + 20)
                .context("pack idx oid table entry out of bounds")?;
            match entry.cmp(oid.as_slice()) {
                std::cmp::Ordering::Equal => {
                    found = Some(m);
                    break;
                }
                std::cmp::Ordering::Less => l = m + 1,
                std::cmp::Ordering::Greater => r = m,
            }
        }
        found.context("OID not in this pack")?
    };

    // Read 4-byte offset (MSB set -> index into the 8-byte large table).
    let raw_off = read_u32_be_at(idx, off_table + idx_pos * 4, "pack idx 32-bit offset")?;
    let pack_offset = if raw_off & 0x8000_0000 != 0 {
        let big_idx = (raw_off & 0x7FFF_FFFF) as usize;
        let big_off = big_table
            .checked_add(
                big_idx
                    .checked_mul(8)
                    .context("pack idx big-offset index overflow")?,
            )
            .context("pack idx big-offset pointer overflow")?;
        usize::try_from(read_u64_be_at(idx, big_off, "pack idx 64-bit offset")?)
            .context("pack idx 64-bit offset does not fit in usize")?
    } else {
        raw_off as usize
    };

    read_pack_entry(pack, pack_offset, git_dir)
}

fn read_u32_be_at(buf: &[u8], offset: usize, what: &str) -> Result<u32> {
    let bytes = buf
        .get(offset..offset + 4)
        .with_context(|| format!("truncated {what} at offset {offset}"))?;
    let mut arr = [0u8; 4];
    arr.copy_from_slice(bytes);
    Ok(u32::from_be_bytes(arr))
}

fn read_u64_be_at(buf: &[u8], offset: usize, what: &str) -> Result<u64> {
    let bytes = buf
        .get(offset..offset + 8)
        .with_context(|| format!("truncated {what} at offset {offset}"))?;
    let mut arr = [0u8; 8];
    arr.copy_from_slice(bytes);
    Ok(u64::from_be_bytes(arr))
}

/// Decode one object entry from a `.pack` file at `offset`.
fn read_pack_entry(pack: &[u8], offset: usize, git_dir: &Path) -> Result<RawObject> {
    let (obj_type, _size, data_pos) = parse_pack_header(pack, offset)?;
    match obj_type {
        // Non-delta: commit(1) / tree(2) / blob(3) / tag(4)
        1..=4 => {
            let data = zlib_decompress(&pack[data_pos..])?;
            Ok(RawObject {
                kind: type_to_kind(obj_type)?,
                data: Bytes::from(data),
            })
        }
        // OFS_DELTA - base referenced by negative offset within this pack.
        6 => {
            let (neg_off, delta_pos) = parse_ofs_delta_header(pack, data_pos)?;
            let base_off = offset
                .checked_sub(neg_off)
                .context("OFS_DELTA: base offset underflow")?;
            let base = read_pack_entry(pack, base_off, git_dir)?;
            let delta = zlib_decompress(&pack[delta_pos..])?;
            let data = apply_delta(&base.data, &delta)?;
            Ok(RawObject {
                kind: base.kind,
                data: Bytes::from(data),
            })
        }
        // REF_DELTA - base referenced by 20-byte OID (may be in any source).
        7 => {
            let base_oid: GitOid = pack[data_pos..data_pos + 20]
                .try_into()
                .context("REF_DELTA: truncated base OID")?;
            let base = read_object(git_dir, &base_oid)?;
            let delta = zlib_decompress(&pack[data_pos + 20..])?;
            let data = apply_delta(&base.data, &delta)?;
            Ok(RawObject {
                kind: base.kind,
                data: Bytes::from(data),
            })
        }
        _ => bail!("unknown pack object type {obj_type}"),
    }
}

/// Variable-length pack object header -> `(type, uncompressed_size, data_pos)`.
fn parse_pack_header(pack: &[u8], mut pos: usize) -> Result<(u8, usize, usize)> {
    let b = *pack.get(pos).context("truncated pack header")?;
    pos += 1;
    let obj_type = (b >> 4) & 0x07;
    let mut size = (b & 0x0F) as usize;
    let mut shift = 4;
    if b & 0x80 != 0 {
        loop {
            let c = *pack.get(pos).context("truncated pack header")?;
            pos += 1;
            size |= ((c & 0x7F) as usize) << shift;
            shift += 7;
            if c & 0x80 == 0 {
                break;
            }
        }
    }
    Ok((obj_type, size, pos))
}

/// OFS_DELTA negative-offset header -> `(negative_offset, delta_data_pos)`.
fn parse_ofs_delta_header(pack: &[u8], mut pos: usize) -> Result<(usize, usize)> {
    let b = *pack.get(pos).context("truncated OFS_DELTA offset")?;
    pos += 1;
    let mut off = (b & 0x7F) as usize;
    if b & 0x80 != 0 {
        loop {
            let c = *pack.get(pos).context("truncated OFS_DELTA offset")?;
            pos += 1;
            off = ((off + 1) << 7) | (c & 0x7F) as usize;
            if c & 0x80 == 0 {
                break;
            }
        }
    }
    Ok((off, pos))
}

// -- delta application --------------------------------------------------------

/// Apply a Git delta instruction stream to a base payload.
///
/// The binary format (documented in `Documentation/gitformat-pack.txt`):
///   1. source-size (variable-length LE int)
///   2. target-size (variable-length LE int)
///   3. instruction stream  - copy-from-source **or** insert-literal
fn apply_delta(base: &[u8], delta: &[u8]) -> Result<Vec<u8>> {
    let (src_len, mut pos) = read_varint_le(delta, 0)?;
    let (tgt_len, next) = read_varint_le(delta, pos)?;
    pos = next;

    if src_len != base.len() {
        bail!(
            "delta source size mismatch: header says {src_len}, base is {}",
            base.len()
        );
    }

    let mut out = Vec::with_capacity(tgt_len);

    while pos < delta.len() {
        let cmd = delta[pos];
        pos += 1;

        if cmd & 0x80 != 0 {
            // -- copy from base -----------------------------------------
            let mut cp_off = 0usize;
            let mut cp_len = 0usize;
            for i in 0..4u8 {
                if cmd & (1 << i) != 0 {
                    cp_off |= (delta[pos] as usize) << (i as usize * 8);
                    pos += 1;
                }
            }
            for i in 0..3u8 {
                if cmd & (1 << (4 + i)) != 0 {
                    cp_len |= (delta[pos] as usize) << (i as usize * 8);
                    pos += 1;
                }
            }
            if cp_len == 0 {
                cp_len = 0x10000;
            }
            out.extend_from_slice(
                base.get(cp_off..cp_off + cp_len)
                    .context("delta copy out of bounds")?,
            );
        } else if cmd != 0 {
            // -- insert literal bytes -----------------------------------
            let n = cmd as usize;
            out.extend_from_slice(
                delta
                    .get(pos..pos + n)
                    .context("delta insert out of bounds")?,
            );
            pos += n;
        } else {
            bail!("reserved zero delta opcode");
        }
    }

    if out.len() != tgt_len {
        bail!(
            "delta target size mismatch: expected {tgt_len}, got {}",
            out.len()
        );
    }
    Ok(out)
}

/// Read a variable-length little-endian integer from a delta stream.
fn read_varint_le(buf: &[u8], mut pos: usize) -> Result<(usize, usize)> {
    let mut val = 0usize;
    let mut shift = 0u32;
    loop {
        let b = *buf.get(pos).context("truncated varint")?;
        pos += 1;
        val |= ((b & 0x7F) as usize) << shift;
        shift += 7;
        if b & 0x80 == 0 {
            break;
        }
    }
    Ok((val, pos))
}

// -- commit parsing -----------------------------------------------------------

fn parse_commit(oid: &GitOid, data: &[u8]) -> Result<GitCommit> {
    let text = std::str::from_utf8(data).context("commit body is not UTF-8")?;

    let mut tree = [0u8; 20];
    let mut parents = Vec::new();
    let mut author_name = String::new();
    let mut author_email = String::new();
    let mut author_ts = 0i64;
    let mut committer_name = String::new();
    let mut committer_email = String::new();
    let mut in_body = false;
    let mut msg = String::new();

    for line in text.split('\n') {
        if in_body {
            if !msg.is_empty() {
                msg.push('\n');
            }
            msg.push_str(line);
            continue;
        }
        if line.is_empty() {
            in_body = true;
            continue;
        }
        // Continuation line (multi-line headers like gpgsig) - skip.
        if line.starts_with(' ') {
            continue;
        }
        if let Some(h) = line.strip_prefix("tree ") {
            tree = parse_hex_oid(h.trim())?;
        } else if let Some(h) = line.strip_prefix("parent ") {
            parents.push(parse_hex_oid(h.trim())?);
        } else if let Some(rest) = line.strip_prefix("author ") {
            let (n, e, t) = parse_ident(rest)?;
            author_name = n;
            author_email = e;
            author_ts = t;
        } else if let Some(rest) = line.strip_prefix("committer ") {
            let (n, e, _) = parse_ident(rest)?;
            committer_name = n;
            committer_email = e;
        }
        // encoding, mergetag, etc. - ignored
    }

    Ok(GitCommit {
        oid: *oid,
        tree,
        parents,
        author_name,
        author_email,
        author_timestamp: author_ts,
        committer_name,
        committer_email,
        message: msg.trim_end().to_string(),
    })
}

/// Parse `Name <email> timestamp tz` identity string.
fn parse_ident(s: &str) -> Result<(String, String, i64)> {
    let lt = s.find('<').context("ident: missing '<'")?;
    let gt = s.find('>').context("ident: missing '>'")?;
    let name = s[..lt].trim().to_string();
    let email = s[lt + 1..gt].to_string();
    let ts: i64 = s[gt + 1..]
        .split_whitespace()
        .next()
        .unwrap_or("0")
        .parse()
        .unwrap_or(0);
    Ok((name, email, ts))
}

// -- helpers ------------------------------------------------------------------

fn parse_hex_oid(hex: &str) -> Result<GitOid> {
    if hex.len() != 40 {
        bail!("invalid OID length {} (expected 40)", hex.len());
    }
    let mut oid = [0u8; 20];
    for i in 0..20 {
        oid[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)?;
    }
    Ok(oid)
}

fn parse_obj_kind(s: &str) -> Result<ObjKind> {
    match s {
        "commit" => Ok(ObjKind::Commit),
        "tree" => Ok(ObjKind::Tree),
        "blob" => Ok(ObjKind::Blob),
        "tag" => Ok(ObjKind::Tag),
        other => bail!("unknown object kind '{other}'"),
    }
}

fn type_to_kind(t: u8) -> Result<ObjKind> {
    match t {
        1 => Ok(ObjKind::Commit),
        2 => Ok(ObjKind::Tree),
        3 => Ok(ObjKind::Blob),
        4 => Ok(ObjKind::Tag),
        _ => bail!("invalid pack type {t}"),
    }
}

fn zlib_decompress(data: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(data).read_to_end(&mut out)?;
    Ok(out)
}

// -- Tree & Blob Extraction Layer ---------------------------------------------

/// A single entry inside a Git tree object.
///
/// Each entry corresponds to either a file (`blob`) or a subdirectory
/// (`tree`).  The `mode` string follows Git conventions:
///
/// | Mode     | Meaning                          |
/// |----------|----------------------------------|
/// | `100644` | Regular file                     |
/// | `100755` | Executable file                  |
/// | `120000` | Symbolic link                    |
/// | `040000` | Subdirectory (another tree)      |
/// | `160000` | Gitlink (submodule)              |
#[derive(Debug, Clone)]
pub struct TreeEntry {
    /// Octal mode as a UTF-8 string (e.g. `"100644"`).
    pub mode: String,
    /// File or directory name (not a full path).
    pub name: String,
    /// 40-char lowercase hex SHA-1 of the pointed-to object.
    pub oid: String,
}

/// Structured representation of a Git tree object.
#[derive(Debug, Clone)]
pub struct GitTree {
    /// All file and directory entries listed in this tree.
    pub entries: Vec<TreeEntry>,
}

/// Parses raw Git tree object bytes into a [`GitTree`].
///
/// Git tree format (binary, no terminating newline):
/// ```text
/// <mode SP <name> NUL <20-byte-binary-OID>  (repeated)
/// ```
///
/// # Errors
/// Returns an error only on I/O-level anomalies; a truncated trailing
/// entry is silently skipped (same behaviour as `git cat-file -p`).
pub fn parse_tree(raw_data: &[u8]) -> Result<GitTree> {
    let mut entries = Vec::new();
    let mut i = 0;

    while i < raw_data.len() {
        // -- mode (terminated by SP) -----------------------------------
        let space_idx = match raw_data[i..].iter().position(|&b| b == b' ') {
            Some(rel) => i + rel,
            None => break, // malformed - stop gracefully
        };
        let mode = String::from_utf8_lossy(&raw_data[i..space_idx]).into_owned();

        // -- name (terminated by NUL) ----------------------------------
        let name_start = space_idx + 1;
        let null_idx = match raw_data[name_start..].iter().position(|&b| b == 0) {
            Some(rel) => name_start + rel,
            None => break,
        };
        let name = String::from_utf8_lossy(&raw_data[name_start..null_idx]).into_owned();

        // -- 20-byte binary OID ----------------------------------------
        let oid_start = null_idx + 1;
        let oid_end = oid_start + 20;
        if oid_end > raw_data.len() {
            break; // truncated entry - stop gracefully
        }
        let oid =
            raw_data[oid_start..oid_end]
                .iter()
                .fold(String::with_capacity(40), |mut s, b| {
                    use std::fmt::Write;
                    let _ = write!(s, "{b:02x}");
                    s
                });

        entries.push(TreeEntry { mode, name, oid });
        i = oid_end;
    }

    Ok(GitTree { entries })
}

/// Reads the blob (file content) for `oid` from the repository at
/// `git_dir` and returns the raw bytes.
///
/// This is the bridge between the Git DAG and the Tree-sitter AST
/// engine: call [`parse_tree`] on a commit's `tree` OID to enumerate
/// files, then call `read_blob` on each file's OID to obtain the bytes
/// that feed into `tree_sitter::Parser`.
pub fn read_blob(git_dir: &Path, oid: &GitOid) -> Result<Vec<u8>> {
    let obj = read_object(git_dir, oid)?;
    if obj.kind != ObjKind::Blob {
        bail!("object {} is not a blob", oid_hex(oid));
    }
    Ok(obj.data.to_vec())
}

/// Recursively extract all blob entries from a Git tree into a flat map.
///
/// Keys are repo-relative slash-separated paths, e.g. `"src/main.rs"`.
/// Subdirectories are traversed recursively.  Gitlinks (submodules) are
/// skipped.  Symlinks (mode `120000`) are stored as raw bytes.
///
/// Call this with `prefix = ""` for the root tree of a commit.
pub fn extract_tree_to_memory(
    git_dir: &Path,
    tree_oid: &GitOid,
    prefix: &str,
    out: &mut HashMap<String, Vec<u8>>,
) -> Result<()> {
    let obj = read_object(git_dir, tree_oid)?;
    if obj.kind != ObjKind::Tree {
        bail!("object {} is not a tree", oid_hex(tree_oid));
    }
    let tree = parse_tree(&obj.data)?;
    for entry in &tree.entries {
        let path = if prefix.is_empty() {
            entry.name.clone()
        } else {
            format!("{prefix}/{}", entry.name)
        };
        let oid = parse_hex_oid(&entry.oid)?;
        if entry.mode == "40000" || entry.mode == "040000" {
            // Subdirectory - recurse.
            extract_tree_to_memory(git_dir, &oid, &path, out)?;
        } else if entry.mode.starts_with("100") || entry.mode.starts_with("120") {
            // Regular file (100644 / 100755) or symlink (120000).
            let blob_obj = read_object(git_dir, &oid)?;
            if blob_obj.kind == ObjKind::Blob {
                out.insert(path, blob_obj.data.to_vec());
            }
        }
        // mode "160000" (gitlink/submodule) - skip.
    }
    Ok(())
}

/// Return all local branch names and their tip commit OIDs.
///
/// Reads loose refs from `refs/heads/` and then fills in any additional
/// branches from `packed-refs` without overwriting loose entries.
pub fn list_branch_heads(path: &Path) -> Result<HashMap<String, GitOid>> {
    let git_dir = resolve_git_dir(path)?;
    let mut branches: HashMap<String, GitOid> = HashMap::new();

    // Loose refs
    let heads_dir = git_dir.join("refs").join("heads");
    if heads_dir.is_dir() {
        collect_loose_refs(&heads_dir, &heads_dir, &mut branches)?;
    }

    // packed-refs (only fills gaps - loose refs take priority)
    let packed = git_dir.join("packed-refs");
    if packed.is_file() {
        for line in std::fs::read_to_string(&packed)?.lines() {
            if line.starts_with('#') || line.starts_with('^') {
                continue;
            }
            let mut parts = line.splitn(2, ' ');
            if let (Some(hex), Some(refname)) = (parts.next(), parts.next())
                && let Some(branch) = refname.strip_prefix("refs/heads/")
                && let Ok(oid) = parse_hex_oid(hex)
            {
                branches.entry(branch.to_string()).or_insert(oid);
            }
        }
    }

    Ok(branches)
}

/// Recursively collect loose ref files under `dir` into `out`.
///
/// Keys are slash-separated paths relative to `base` (the
/// `refs/heads/` directory), e.g. `"main"` or `"feature/fix-42"`.
fn collect_loose_refs(base: &Path, dir: &Path, out: &mut HashMap<String, GitOid>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_loose_refs(base, &path, out)?;
        } else if path.is_file() {
            let rel = path.strip_prefix(base)?;
            let name = rel.to_string_lossy().replace('\\', "/");
            let hex = std::fs::read_to_string(&path)?;
            if let Ok(oid) = parse_hex_oid(hex.trim()) {
                out.insert(name, oid);
            }
        }
    }
    Ok(())
}

/// Read `user.name` and `user.email` from a Git repository's config INI file.
///
/// Searches `<repo>/.git/config` first, then falls back to `~/.gitconfig`.
/// Returns `None` if neither file contains a `[user]` section with both fields.
pub fn read_git_user_config(repo_path: &Path) -> Option<(String, String)> {
    // Prefer the local repo config, fall back to the global gitconfig.
    let candidates: Vec<PathBuf> = {
        let mut v = Vec::new();
        if let Ok(git_dir) = resolve_git_dir(repo_path) {
            v.push(git_dir.join("config"));
        }
        if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
            v.push(PathBuf::from(home).join(".gitconfig"));
        }
        v
    };

    for path in candidates {
        if let Ok(text) = std::fs::read_to_string(&path)
            && let Some(pair) = parse_git_user_config(&text)
        {
            return Some(pair);
        }
    }
    None
}

/// Parse `user.name` and `user.email` from a Git INI config string.
fn parse_git_user_config(text: &str) -> Option<(String, String)> {
    let mut in_user = false;
    let mut name: Option<String> = None;
    let mut email: Option<String> = None;

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.starts_with('[') {
            in_user = line.to_lowercase().starts_with("[user]");
            continue;
        }
        if !in_user {
            continue;
        }
        if let Some(rest) = line.strip_prefix("name")
            && let Some(v) = rest.trim_start().strip_prefix('=')
        {
            name = Some(v.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("email")
            && let Some(v) = rest.trim_start().strip_prefix('=')
        {
            email = Some(v.trim().to_string());
        }
    }

    match (name, email) {
        (Some(n), Some(e)) => Some((n, e)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::process::Command;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    fn backend_override_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn git(args: &[&str], dir: &Path) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .status()
            .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
        assert!(status.success(), "git {args:?} failed with {status}");
    }

    fn git_output(args: &[&str], dir: &Path) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
        assert!(
            out.status.success(),
            "git {args:?} failed with {}",
            out.status
        );
        String::from_utf8(out.stdout)
            .unwrap_or_else(|e| panic!("git {args:?} produced non-utf8 output: {e}"))
    }

    struct BackendOverrideGuard {
        previous_pack: u8,
        previous_traversal: u8,
        _lock: MutexGuard<'static, ()>,
    }

    impl BackendOverrideGuard {
        fn set(mode: u8) -> Self {
            Self::set_with(mode, TEST_TRAVERSAL_AUTO)
        }

        fn set_with(pack_mode: u8, traversal_mode: u8) -> Self {
            let lock = backend_override_lock()
                .lock()
                .expect("backend override mutex should not be poisoned");
            let previous_pack = set_test_backend_override(pack_mode);
            let previous_traversal = set_test_traversal_override(traversal_mode);
            Self {
                previous_pack,
                previous_traversal,
                _lock: lock,
            }
        }
    }

    impl Drop for BackendOverrideGuard {
        fn drop(&mut self) {
            let _ = set_test_backend_override(self.previous_pack);
            let _ = set_test_traversal_override(self.previous_traversal);
        }
    }

    fn create_packed_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();

        git(&["init"], path);
        git(&["config", "user.email", "test@test.com"], path);
        git(&["config", "user.name", "test"], path);
        git(&["config", "core.autocrlf", "false"], path);

        for i in 0..8 {
            std::fs::write(path.join(format!("f{i}.txt")), format!("line-{i}\n")).unwrap();
            git(&["add", "."], path);
            git(&["commit", "-m", &format!("commit-{i}")], path);
        }

        std::fs::write(path.join("binary.bin"), vec![0x00, 0xff, 0x10, 0x80, 0x01]).unwrap();
        git(&["add", "."], path);
        git(&["commit", "-m", "binary"], path);

        git(&["gc", "--aggressive", "--prune=now"], path);
        dir
    }

    fn create_commit_graph_repo() -> tempfile::TempDir {
        let dir = create_packed_repo();
        git(&["commit-graph", "write", "--reachable"], dir.path());
        dir
    }

    /// `analyze_git_repo` must return the correct commit count and metadata.
    #[test]
    fn test_analyze_git_repo_basic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();

        git(&["init"], path);
        git(&["config", "user.email", "test@test.com"], path);
        git(&["config", "user.name", "test"], path);
        git(&["config", "core.autocrlf", "false"], path);
        std::fs::write(path.join("a.rs"), "fn a() {}").unwrap();
        git(&["add", "."], path);
        git(&["commit", "-m", "first commit"], path);

        let analysis = analyze_git_repo(path).unwrap();

        assert_eq!(analysis.commit_count, 1, "must report exactly 1 commit");
        assert_eq!(analysis.commits.len(), 1);
        assert_eq!(analysis.commits[0].message, "first commit");
        assert_eq!(analysis.head_hex.len(), 40, "HEAD hex must be 40 chars");
    }

    /// `extract_tree_to_memory` must return the exact bytes for all files in
    /// the tree, including files in subdirectories.
    #[test]
    fn test_extract_tree_to_memory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();

        git(&["init"], path);
        git(&["config", "user.email", "test@test.com"], path);
        git(&["config", "user.name", "test"], path);
        git(&["config", "core.autocrlf", "false"], path);

        std::fs::write(path.join("root.rs"), b"fn root() {}" as &[u8]).unwrap();
        std::fs::create_dir_all(path.join("sub")).unwrap();
        std::fs::write(
            path.join("sub").join("nested.rs"),
            b"fn nested() {}" as &[u8],
        )
        .unwrap();

        git(&["add", "."], path);
        git(&["commit", "-m", "initial"], path);

        let analysis = analyze_git_repo(path).unwrap();
        let git_dir = resolve_git_dir(path).unwrap();
        let tree_oid = analysis.commits[0].tree;

        let mut files: HashMap<String, Vec<u8>> = HashMap::new();
        extract_tree_to_memory(&git_dir, &tree_oid, "", &mut files).unwrap();

        assert!(
            files.contains_key("root.rs"),
            "root-level file must be extracted"
        );
        assert!(
            files.contains_key("sub/nested.rs"),
            "nested file must be extracted"
        );
        assert_eq!(
            files["root.rs"], b"fn root() {}",
            "root.rs bytes must match exactly"
        );
        assert_eq!(
            files["sub/nested.rs"], b"fn nested() {}",
            "sub/nested.rs bytes must match exactly"
        );
    }

    /// `list_branch_heads` must return the correct branch name and its tip OID.
    #[test]
    fn test_list_branch_heads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();

        git(&["init"], path);
        git(&["config", "user.email", "test@test.com"], path);
        git(&["config", "user.name", "test"], path);
        git(&["config", "core.autocrlf", "false"], path);
        std::fs::write(path.join("a.rs"), "fn a() {}").unwrap();
        git(&["add", "."], path);
        git(&["commit", "-m", "first"], path);

        let heads = list_branch_heads(path).unwrap();
        assert_eq!(heads.len(), 1, "must find exactly one branch");

        let branch_name = heads.keys().next().unwrap();
        assert!(
            branch_name == "main" || branch_name == "master",
            "branch name must be main or master, got: {branch_name}"
        );

        // The branch tip OID must match the HEAD reported by analyze_git_repo.
        let analysis = analyze_git_repo(path).unwrap();
        let tip_hex = oid_hex(heads.values().next().unwrap());
        assert_eq!(tip_hex, analysis.head_hex, "branch tip OID must equal HEAD");
    }

    /// Packed repositories should be ingested through the mmap-first index path
    /// while preserving commit ordering and metadata parity.
    #[test]
    fn test_analyze_git_repo_with_packed_objects() {
        let dir = create_packed_repo();
        let path = dir.path();

        let git_dir = resolve_git_dir(path).unwrap();
        let pack_dir = git_dir.join("objects").join("pack");
        let has_idx = std::fs::read_dir(&pack_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().ends_with(".idx"));
        assert!(
            has_idx,
            "git gc should produce at least one pack index file"
        );

        let head_hex = git_output(&["rev-parse", "HEAD"], path).trim().to_string();
        let analysis = analyze_git_repo(path).unwrap();

        assert_eq!(
            analysis.head_hex, head_hex,
            "head OID must match git rev-parse"
        );
        assert_eq!(
            analysis.commit_count, 9,
            "all reachable commits must be returned"
        );
        assert_eq!(
            analysis.commits.first().unwrap().message,
            "commit-0",
            "oldest-first ordering must be preserved after packing"
        );
        assert_eq!(
            analysis.commits.last().unwrap().message,
            "binary",
            "latest commit must remain at the end"
        );

        let mut files: HashMap<String, Vec<u8>> = HashMap::new();
        let tree = analysis.commits.last().unwrap().tree;
        extract_tree_to_memory(&git_dir, &tree, "", &mut files).unwrap();
        assert_eq!(
            files.get("binary.bin").unwrap(),
            &vec![0x00, 0xff, 0x10, 0x80, 0x01],
            "binary payload must round-trip exactly through packed object decode"
        );
    }

    #[test]
    fn test_mmap_only_backend_on_packed_repo() {
        let _guard = BackendOverrideGuard::set(TEST_BACKEND_MMAP_ONLY);
        let dir = create_packed_repo();
        let analysis = analyze_git_repo(dir.path()).unwrap();
        assert_eq!(analysis.commit_count, 9);
        assert_eq!(analysis.commits.last().unwrap().message, "binary");
    }

    #[test]
    fn test_forced_mmap_failure_falls_back_to_legacy() {
        let _guard = BackendOverrideGuard::set(TEST_BACKEND_FORCE_MMAP_FAIL);
        let dir = create_packed_repo();
        let analysis = analyze_git_repo(dir.path()).unwrap();
        assert_eq!(analysis.commit_count, 9);
        assert_eq!(analysis.commits.last().unwrap().message, "binary");
    }

    #[test]
    fn test_mmap_and_legacy_backends_are_parity_equivalent() {
        let dir = create_packed_repo();

        let mmap_analysis = {
            let _guard = BackendOverrideGuard::set(TEST_BACKEND_MMAP_ONLY);
            analyze_git_repo(dir.path()).unwrap()
        };

        let legacy_analysis = {
            let _guard = BackendOverrideGuard::set(TEST_BACKEND_LEGACY_ONLY);
            analyze_git_repo(dir.path()).unwrap()
        };

        assert_eq!(mmap_analysis.head_hex, legacy_analysis.head_hex);
        assert_eq!(mmap_analysis.commit_count, legacy_analysis.commit_count);
        let mmap_messages: Vec<&str> = mmap_analysis
            .commits
            .iter()
            .map(|c| c.message.as_str())
            .collect();
        let legacy_messages: Vec<&str> = legacy_analysis
            .commits
            .iter()
            .map(|c| c.message.as_str())
            .collect();
        assert_eq!(mmap_messages, legacy_messages);
    }

    #[test]
    fn test_commit_graph_and_legacy_traversal_are_parity_equivalent() {
        let dir = create_commit_graph_repo();

        let graph_analysis = {
            let _guard =
                BackendOverrideGuard::set_with(TEST_BACKEND_AUTO, TEST_TRAVERSAL_COMMIT_GRAPH_ONLY);
            analyze_git_repo(dir.path()).unwrap()
        };

        let legacy_analysis = {
            let _guard =
                BackendOverrideGuard::set_with(TEST_BACKEND_AUTO, TEST_TRAVERSAL_LEGACY_ONLY);
            analyze_git_repo(dir.path()).unwrap()
        };

        assert_eq!(graph_analysis.head_hex, legacy_analysis.head_hex);
        assert_eq!(graph_analysis.commit_count, legacy_analysis.commit_count);
        let graph_oids: Vec<GitOid> = graph_analysis.commits.iter().map(|c| c.oid).collect();
        let legacy_oids: Vec<GitOid> = legacy_analysis.commits.iter().map(|c| c.oid).collect();
        assert_eq!(graph_oids, legacy_oids);
        let graph_messages: Vec<&str> = graph_analysis
            .commits
            .iter()
            .map(|c| c.message.as_str())
            .collect();
        let legacy_messages: Vec<&str> = legacy_analysis
            .commits
            .iter()
            .map(|c| c.message.as_str())
            .collect();
        assert_eq!(graph_messages, legacy_messages);
    }

    #[test]
    fn test_stale_commit_graph_falls_back_to_legacy() {
        let dir = create_commit_graph_repo();
        let path = dir.path();

        std::fs::write(path.join("post_graph.txt"), "latest\n").unwrap();
        git(&["add", "."], path);
        git(&["commit", "-m", "post-graph"], path);

        let analysis = analyze_git_repo(path).unwrap();
        assert_eq!(analysis.commits.last().unwrap().message, "post-graph");

        let legacy = {
            let _guard =
                BackendOverrideGuard::set_with(TEST_BACKEND_AUTO, TEST_TRAVERSAL_LEGACY_ONLY);
            analyze_git_repo(path).unwrap()
        };
        assert_eq!(analysis.head_hex, legacy.head_hex);
        assert_eq!(analysis.commit_count, legacy.commit_count);
        let analysis_messages: Vec<&str> = analysis
            .commits
            .iter()
            .map(|c| c.message.as_str())
            .collect();
        let legacy_messages: Vec<&str> =
            legacy.commits.iter().map(|c| c.message.as_str()).collect();
        assert_eq!(analysis_messages, legacy_messages);
    }
}
