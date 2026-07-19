use anyhow::{Context, Result, bail};
use bytes::Bytes;
use memmap2::{Mmap, MmapOptions};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::File;
use std::path::{Path, PathBuf};

use crate::domain::{
    apply_delta, oid_hex, parse_commit, parse_git_user_config, parse_hex_oid, parse_obj_kind,
    parse_ofs_delta_header, parse_pack_header, parse_tree, type_to_kind, zlib_decompress,
};
use crate::{GitAnalysis, GitCommit, GitOid, ObjKind, RawObject};

pub(crate) const TEST_TRAVERSAL_AUTO: u8 = 0;
pub(crate) const TEST_TRAVERSAL_COMMIT_GRAPH_ONLY: u8 = 1;
pub(crate) const TEST_TRAVERSAL_LEGACY_ONLY: u8 = 2;
pub(crate) const TEST_TRAVERSAL_FORCE_GRAPH_FAIL: u8 = 3;

#[cfg(test)]
thread_local! {
    static TEST_TRAVERSAL_OVERRIDE: std::cell::Cell<u8> = const { std::cell::Cell::new(TEST_TRAVERSAL_AUTO) };
}

#[cfg(test)]
pub(crate) fn set_test_traversal_override(mode: u8) -> u8 {
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

/// Open the Git repository at `path`, walk every reachable commit from
/// HEAD, and return structured analysis with commits **oldest-first**.
pub fn analyze_git_repo(path: &Path) -> Result<GitAnalysis> {
    let git_dir = resolve_git_dir(path)?;
    let head_oid = resolve_head(&git_dir)?;
    let head_hex = oid_hex(&head_oid);

    let commits = collect_commits_with_fallback(&git_dir, head_oid)?;

    Ok(GitAnalysis { path: path.to_path_buf(), head_hex, commit_count: commits.len(), commits })
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
        commits.push(parse_commit(&oid, &obj.data)?);
    }

    Ok(commits)
}

/// Locate the `.git` directory for the repository rooted at `path`.
///
/// Handles plain repositories (`.git/` subdirectory), worktrees
/// (`.git` file with a `gitdir:` pointer), and bare repositories.
pub fn resolve_git_dir(path: &Path) -> Result<PathBuf> {
    let dot_git = path.join(".git");
    if dot_git.is_dir() {
        return Ok(dot_git);
    }
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

fn resolve_ref(git_dir: &Path, refpath: &str) -> Result<GitOid> {
    let loose = git_dir.join(refpath);
    if loose.is_file() {
        let hex = std::fs::read_to_string(&loose)?;
        return parse_hex_oid(hex.trim());
    }
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
    let nul =
        buf.iter().position(|&b| b == 0).context("malformed loose object: no NUL separator")?;
    let header = std::str::from_utf8(&buf[..nul])?;
    let kind = parse_obj_kind(header.split(' ').next().unwrap_or(""))?;
    Ok(RawObject { kind, data: Bytes::copy_from_slice(&buf[nul + 1..]) })
}

#[derive(Debug, Clone, Copy)]
enum PackLookupBackend {
    MmapChunkIndex,
    LegacyIndexScan,
}

pub(crate) const TEST_BACKEND_AUTO: u8 = 0;
pub(crate) const TEST_BACKEND_MMAP_ONLY: u8 = 1;
pub(crate) const TEST_BACKEND_LEGACY_ONLY: u8 = 2;
pub(crate) const TEST_BACKEND_FORCE_MMAP_FAIL: u8 = 3;

#[cfg(test)]
thread_local! {
    static TEST_BACKEND_OVERRIDE: std::cell::Cell<u8> = const { std::cell::Cell::new(TEST_BACKEND_AUTO) };
}

#[cfg(test)]
pub(crate) fn set_test_backend_override(mode: u8) -> u8 {
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

fn read_packed_object(git_dir: &Path, oid: &GitOid) -> Result<RawObject> {
    #[cfg(test)]
    let mode = get_test_backend_override();
    #[cfg(not(test))]
    let mode = TEST_BACKEND_AUTO;

    let backends: &[PackLookupBackend] = match mode {
        TEST_BACKEND_MMAP_ONLY => &[PackLookupBackend::MmapChunkIndex],
        TEST_BACKEND_LEGACY_ONLY => &[PackLookupBackend::LegacyIndexScan],
        _ => &[PackLookupBackend::MmapChunkIndex, PackLookupBackend::LegacyIndexScan],
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

const COMMIT_GRAPH_SIGNATURE: &[u8; 4] = b"CGPH";
const COMMIT_GRAPH_HEADER_LEN: usize = 8;
const COMMIT_GRAPH_FAN_LEN: usize = 256;
const COMMIT_GRAPH_COMMIT_DATA_SANS_HASH: usize = 16;
const COMMIT_GRAPH_NO_PARENT: u32 = 0x7000_0000;
const COMMIT_GRAPH_EXTENDED_EDGE_MASK: u32 = 0x8000_0000;

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
    fn open(git_dir: &Path) -> Result<Self> {
        let path = resolve_commit_graph_path(git_dir)?;
        let data = mmap_read_only(&path)?;
        Self::from_mmap(data)
    }

    fn from_mmap(data: Mmap) -> Result<Self> {
        if data.len() < COMMIT_GRAPH_HEADER_LEN {
            bail!("commit-graph is too small");
        }

        if data.get(0..4).context("commit-graph header missing signature")?
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
        let base_graph_count = *data.get(7).context("commit-graph missing base graph count")?;
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
            let offset =
                usize::try_from(read_u64_be_at(&data, cursor + 4, "commit-graph chunk offset")?)
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

        let oidf = chunks.get(b"OIDF").context("commit-graph missing OIDF chunk")?.clone();
        let oidl = chunks.get(b"OIDL").context("commit-graph missing OIDL chunk")?.clone();
        let cdat = chunks.get(b"CDAT").context("commit-graph missing CDAT chunk")?.clone();
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

        let expected_oidl =
            commit_count_usize.checked_mul(hash_len).context("commit-graph OIDL size overflow")?;
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
        parent_positions.into_iter().map(|p| self.oid_at(p)).collect()
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
            cursor = cursor.checked_add(4).context("commit-graph EDGE cursor overflow")?;

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

fn lookup_in_pack(idx: &[u8], pack: &[u8], oid: &GitOid, git_dir: &Path) -> Result<RawObject> {
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

    let oid_table: usize = 1032;
    let crc_table = oid_table
        .checked_add(total.checked_mul(20).context("pack idx oid table overflow")?)
        .context("pack idx crc table overflow")?;
    let off_table = crc_table
        .checked_add(total.checked_mul(4).context("pack idx crc span overflow")?)
        .context("pack idx offset table overflow")?;
    let big_table = off_table
        .checked_add(total.checked_mul(4).context("pack idx offset span overflow")?)
        .context("pack idx big-offset table overflow")?;
    if big_table > idx.len() {
        bail!("pack idx tables exceed file length");
    }

    let idx_pos = {
        let (mut l, mut r) = (lo, hi);
        let mut found = None;
        while l < r {
            let m = l + (r - l) / 2;
            let start = oid_table + m * 20;
            let entry =
                idx.get(start..start + 20).context("pack idx oid table entry out of bounds")?;
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

    let raw_off = read_u32_be_at(idx, off_table + idx_pos * 4, "pack idx 32-bit offset")?;
    let pack_offset = if raw_off & 0x8000_0000 != 0 {
        let big_idx = (raw_off & 0x7FFF_FFFF) as usize;
        let big_off = big_table
            .checked_add(big_idx.checked_mul(8).context("pack idx big-offset index overflow")?)
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

fn read_pack_entry(pack: &[u8], offset: usize, git_dir: &Path) -> Result<RawObject> {
    let (obj_type, _size, data_pos) = parse_pack_header(pack, offset)?;
    match obj_type {
        1..=4 => {
            let data = zlib_decompress(&pack[data_pos..])?;
            Ok(RawObject { kind: type_to_kind(obj_type)?, data: Bytes::from(data) })
        }
        6 => {
            let (neg_off, delta_pos) = parse_ofs_delta_header(pack, data_pos)?;
            let base_off =
                offset.checked_sub(neg_off).context("OFS_DELTA: base offset underflow")?;
            let base = read_pack_entry(pack, base_off, git_dir)?;
            let delta = zlib_decompress(&pack[delta_pos..])?;
            let data = apply_delta(&base.data, &delta)?;
            Ok(RawObject { kind: base.kind, data: Bytes::from(data) })
        }
        7 => {
            let base_oid: GitOid = pack
                .get(data_pos..data_pos + 20)
                .context("REF_DELTA: truncated base OID")?
                .try_into()
                .context("REF_DELTA: invalid base OID length")?;
            let base = read_object(git_dir, &base_oid)?;
            let delta = zlib_decompress(
                pack.get(data_pos + 20..).context("REF_DELTA: truncated delta payload")?,
            )?;
            let data = apply_delta(&base.data, &delta)?;
            Ok(RawObject { kind: base.kind, data: Bytes::from(data) })
        }
        _ => bail!("unknown pack object type {obj_type}"),
    }
}

pub fn read_blob(git_dir: &Path, oid: &GitOid) -> Result<Vec<u8>> {
    let obj = read_object(git_dir, oid)?;
    if obj.kind != ObjKind::Blob {
        bail!("object {} is not a blob", oid_hex(oid));
    }
    Ok(obj.data.to_vec())
}

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
        let path =
            if prefix.is_empty() { entry.name.clone() } else { format!("{prefix}/{}", entry.name) };
        let oid = parse_hex_oid(&entry.oid)?;
        if entry.mode == "40000" || entry.mode == "040000" {
            extract_tree_to_memory(git_dir, &oid, &path, out)?;
        } else if entry.mode.starts_with("100") || entry.mode.starts_with("120") {
            let blob_obj = read_object(git_dir, &oid)?;
            if blob_obj.kind == ObjKind::Blob {
                out.insert(path, blob_obj.data.to_vec());
            }
        }
    }
    Ok(())
}

pub fn list_branch_heads(path: &Path) -> Result<HashMap<String, GitOid>> {
    let git_dir = resolve_git_dir(path)?;
    let mut branches: HashMap<String, GitOid> = HashMap::new();

    let heads_dir = git_dir.join("refs").join("heads");
    if heads_dir.is_dir() {
        collect_loose_refs(&heads_dir, &heads_dir, &mut branches)?;
    }

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

pub fn read_git_user_config(repo_path: &Path) -> Option<(String, String)> {
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
