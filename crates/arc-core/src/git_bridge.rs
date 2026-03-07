//! Pure‑Rust Git history reader.
//!
//! Opens a legacy Git repository, decompresses loose and packed objects,
//! parses commit metadata, and walks the DAG from HEAD — without linking
//! to libgit2 or depending on the `gix` crate.  Only [`flate2`] is used
//! for zlib decompression.
//!
//! # Design (inspired by gitoxide)
//!
//! The `gix` project splits Git I/O across many fine‑grained crates
//! (`gix-odb`, `gix-pack`, `gix-object`, `gix-revwalk`, …).  We follow
//! the same logical layering inside a single module: ref resolution →
//! object I/O (loose + pack) → commit parsing → DAG traversal.

use anyhow::{bail, Context, Result};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Read;
use std::path::{Path, PathBuf};

// ── types ──────────────────────────────────────────────────────────────

/// A 20‑byte SHA‑1 object identifier — Git's native hash format.
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
struct RawObject {
    kind: ObjKind,
    data: Vec<u8>,
}

/// Parsed metadata extracted from a single Git commit object.
#[derive(Debug, Clone)]
pub struct GitCommit {
    /// SHA‑1 hash of this commit.
    pub oid: GitOid,
    /// SHA‑1 of the root tree object.
    pub tree: GitOid,
    /// Parent commit OIDs (empty for root commits).
    pub parents: Vec<GitOid>,
    /// Author name.
    pub author_name: String,
    /// Author email.
    pub author_email: String,
    /// Author‑date as a Unix timestamp (seconds since epoch).
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
    /// HEAD commit as a 40‑char lowercase hex string.
    pub head_hex: String,
    /// Total number of reachable commits.
    pub commit_count: usize,
    /// All reachable commits in topological order, **oldest first**.
    pub commits: Vec<GitCommit>,
}

// ── public API ─────────────────────────────────────────────────────────

/// Open the Git repository at `path`, walk every reachable commit from
/// HEAD, and return structured analysis with commits **oldest‑first**.
pub fn analyze_git_repo(path: &Path) -> Result<GitAnalysis> {
    let git_dir = resolve_git_dir(path)?;
    let head_oid = resolve_head(&git_dir)?;
    let head_hex = oid_hex(&head_oid);

    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    let mut commits = Vec::new();

    queue.push_back(head_oid);
    while let Some(oid) = queue.pop_front() {
        if !visited.insert(oid) {
            continue;
        }
        let obj = read_object(&git_dir, &oid)?;
        if obj.kind != ObjKind::Commit {
            continue;
        }
        let commit = parse_commit(&oid, &obj.data)?;
        for p in &commit.parents {
            queue.push_back(*p);
        }
        commits.push(commit);
    }

    // Reverse BFS order → oldest commit first (natural for replaying).
    commits.reverse();

    Ok(GitAnalysis {
        path: path.to_path_buf(),
        head_hex,
        commit_count: commits.len(),
        commits,
    })
}

/// Render a [`GitOid`] as a 40‑char lowercase hex string.
pub fn oid_hex(oid: &GitOid) -> String {
    oid.iter().map(|b| format!("{b:02x}")).collect()
}

// ── git dir resolution ─────────────────────────────────────────────────

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
    let raw = std::fs::read_to_string(git_dir.join("HEAD"))
        .context("failed to read .git/HEAD")?;
    let raw = raw.trim();
    if let Some(refpath) = raw.strip_prefix("ref: ") {
        resolve_ref(git_dir, refpath)
    } else {
        parse_hex_oid(raw)
    }
}

/// Resolve a ref name (e.g. `refs/heads/main`) → OID.
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

// ── object I/O ─────────────────────────────────────────────────────────

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
        data: buf[nul + 1..].to_vec(),
    })
}

// ── pack files ─────────────────────────────────────────────────────────

fn read_packed_object(git_dir: &Path, oid: &GitOid) -> Result<RawObject> {
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
        if let Ok(obj) = lookup_in_pack(&idx_path, &pack_path, oid, git_dir) {
            return Ok(obj);
        }
    }
    bail!("object not in any pack");
}

/// Look up `oid` in a **v2** pack index, then extract from the `.pack`.
fn lookup_in_pack(
    idx_path: &Path,
    pack_path: &Path,
    oid: &GitOid,
    git_dir: &Path,
) -> Result<RawObject> {
    let idx = std::fs::read(idx_path)?;
    let pack = std::fs::read(pack_path)?;

    // ── v2 index header ─────────────────────────────────────────────
    //  0..4   magic  0xff 't' 'O' 'c'
    //  4..8   version (2)
    //  8..1032  256-entry fan-out table (4 bytes each, big-endian)
    if idx.len() < 1032 || &idx[..4] != b"\xfftOc" {
        bail!("unsupported pack index format");
    }
    let ver = u32::from_be_bytes(idx[4..8].try_into()?);
    if ver != 2 {
        bail!("pack index v{ver} not supported (only v2)");
    }

    let fanout = |i: usize| -> u32 {
        let off = 8 + i * 4;
        u32::from_be_bytes(idx[off..off + 4].try_into().unwrap())
    };
    let total = fanout(255) as usize;

    let fb = oid[0] as usize;
    let lo = if fb == 0 { 0 } else { fanout(fb - 1) as usize };
    let hi = fanout(fb) as usize;

    let oid_table = 1032; // 8 + 256*4
    let crc_table = oid_table + total * 20;
    let off_table = crc_table + total * 4;
    let big_table = off_table + total * 4;

    // Binary search in the sorted OID table [lo, hi).
    let idx_pos = {
        let (mut l, mut r) = (lo, hi);
        let mut found = None;
        while l < r {
            let m = l + (r - l) / 2;
            let start = oid_table + m * 20;
            let entry = &idx[start..start + 20];
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

    // Read 4-byte offset (MSB set → index into the 8-byte large table).
    let raw_off = u32::from_be_bytes(
        idx[off_table + idx_pos * 4..off_table + idx_pos * 4 + 4].try_into()?,
    );
    let pack_offset = if raw_off & 0x8000_0000 != 0 {
        let big_idx = (raw_off & 0x7FFF_FFFF) as usize;
        u64::from_be_bytes(
            idx[big_table + big_idx * 8..big_table + big_idx * 8 + 8].try_into()?,
        ) as usize
    } else {
        raw_off as usize
    };

    read_pack_entry(&pack, pack_offset, git_dir)
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
                data,
            })
        }
        // OFS_DELTA — base referenced by negative offset within this pack.
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
                data,
            })
        }
        // REF_DELTA — base referenced by 20-byte OID (may be in any source).
        7 => {
            let base_oid: GitOid = pack[data_pos..data_pos + 20]
                .try_into()
                .context("REF_DELTA: truncated base OID")?;
            let base = read_object(git_dir, &base_oid)?;
            let delta = zlib_decompress(&pack[data_pos + 20..])?;
            let data = apply_delta(&base.data, &delta)?;
            Ok(RawObject {
                kind: base.kind,
                data,
            })
        }
        _ => bail!("unknown pack object type {obj_type}"),
    }
}

/// Variable‑length pack object header → `(type, uncompressed_size, data_pos)`.
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

/// OFS_DELTA negative‑offset header → `(negative_offset, delta_data_pos)`.
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

// ── delta application ──────────────────────────────────────────────────

/// Apply a Git delta instruction stream to a base payload.
///
/// The binary format (documented in `Documentation/gitformat-pack.txt`):
///   1. source‑size (variable‑length LE int)
///   2. target‑size (variable‑length LE int)
///   3. instruction stream  — copy‑from‑source **or** insert‑literal
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
            // ── copy from base ──────────────────────────────────────
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
            // ── insert literal bytes ────────────────────────────────
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

/// Read a variable‑length little‑endian integer from a delta stream.
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

// ── commit parsing ─────────────────────────────────────────────────────

fn parse_commit(oid: &GitOid, data: &[u8]) -> Result<GitCommit> {
    let text = std::str::from_utf8(data).context("commit body is not UTF‑8")?;

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
        // Continuation line (multi‑line headers like gpgsig) — skip.
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
        // encoding, mergetag, etc. — ignored
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

// ── helpers ────────────────────────────────────────────────────────────

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

// ── Tree & Blob Extraction Layer ─────────────────────────────

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
        // ── mode (terminated by SP) ───────────────────────────────────
        let space_idx = match raw_data[i..].iter().position(|&b| b == b' ') {
            Some(rel) => i + rel,
            None => break, // malformed — stop gracefully
        };
        let mode = String::from_utf8_lossy(&raw_data[i..space_idx]).into_owned();

        // ── name (terminated by NUL) ──────────────────────────────────
        let name_start = space_idx + 1;
        let null_idx = match raw_data[name_start..].iter().position(|&b| b == 0) {
            Some(rel) => name_start + rel,
            None => break,
        };
        let name = String::from_utf8_lossy(&raw_data[name_start..null_idx]).into_owned();

        // ── 20-byte binary OID ────────────────────────────────────────
        let oid_start = null_idx + 1;
        let oid_end = oid_start + 20;
        if oid_end > raw_data.len() {
            break; // truncated entry — stop gracefully
        }
        let oid = raw_data[oid_start..oid_end]
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
    Ok(obj.data)
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
            // Subdirectory — recurse.
            extract_tree_to_memory(git_dir, &oid, &path, out)?;
        } else if entry.mode.starts_with("100") || entry.mode.starts_with("120") {
            // Regular file (100644 / 100755) or symlink (120000).
            let blob_obj = read_object(git_dir, &oid)?;
            if blob_obj.kind == ObjKind::Blob {
                out.insert(path, blob_obj.data);
            }
        }
        // mode "160000" (gitlink/submodule) — skip.
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

    // packed-refs (only fills gaps — loose refs take priority)
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
fn collect_loose_refs(
    base: &Path,
    dir: &Path,
    out: &mut HashMap<String, GitOid>,
) -> Result<()> {
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
