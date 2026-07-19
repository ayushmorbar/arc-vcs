use anyhow::{Context, Result, bail};
use std::io::Read;

use crate::{GitCommit, GitOid, GitTree, ObjKind, TreeEntry};

/// Render a [`GitOid`] as a 40-char lowercase hex string.
pub fn oid_hex(oid: &GitOid) -> String {
    oid.iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn parse_commit(oid: &GitOid, data: &[u8]) -> Result<GitCommit> {
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
    let ts: i64 = s[gt + 1..].split_whitespace().next().unwrap_or("0").parse().unwrap_or(0);
    Ok((name, email, ts))
}

pub(crate) fn parse_hex_oid(hex: &str) -> Result<GitOid> {
    if hex.len() != 40 {
        bail!("invalid OID length {} (expected 40)", hex.len());
    }
    let mut oid = [0u8; 20];
    for i in 0..20 {
        oid[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)?;
    }
    Ok(oid)
}

pub(crate) fn parse_obj_kind(s: &str) -> Result<ObjKind> {
    match s {
        "commit" => Ok(ObjKind::Commit),
        "tree" => Ok(ObjKind::Tree),
        "blob" => Ok(ObjKind::Blob),
        "tag" => Ok(ObjKind::Tag),
        other => bail!("unknown object kind '{other}'"),
    }
}

pub(crate) fn type_to_kind(t: u8) -> Result<ObjKind> {
    match t {
        1 => Ok(ObjKind::Commit),
        2 => Ok(ObjKind::Tree),
        3 => Ok(ObjKind::Blob),
        4 => Ok(ObjKind::Tag),
        _ => bail!("invalid pack type {t}"),
    }
}

pub(crate) fn zlib_decompress(data: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(data).read_to_end(&mut out)?;
    Ok(out)
}

/// Variable-length pack object header -> `(type, uncompressed_size, data_pos)`.
pub(crate) fn parse_pack_header(pack: &[u8], mut pos: usize) -> Result<(u8, usize, usize)> {
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
pub(crate) fn parse_ofs_delta_header(pack: &[u8], mut pos: usize) -> Result<(usize, usize)> {
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

/// Apply a Git delta instruction stream to a base payload.
///
/// The binary format (documented in `Documentation/gitformat-pack.txt`):
///   1. source-size (variable-length LE int)
///   2. target-size (variable-length LE int)
///   3. instruction stream  - copy-from-source **or** insert-literal
pub(crate) fn apply_delta(base: &[u8], delta: &[u8]) -> Result<Vec<u8>> {
    let (src_len, mut pos) = read_varint_le(delta, 0)?;
    let (tgt_len, next) = read_varint_le(delta, pos)?;
    pos = next;

    if src_len != base.len() {
        bail!("delta source size mismatch: header says {src_len}, base is {}", base.len());
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
                base.get(cp_off..cp_off + cp_len).context("delta copy out of bounds")?,
            );
        } else if cmd != 0 {
            // -- insert literal bytes -----------------------------------
            let n = cmd as usize;
            out.extend_from_slice(delta.get(pos..pos + n).context("delta insert out of bounds")?);
            pos += n;
        } else {
            bail!("reserved zero delta opcode");
        }
    }

    if out.len() != tgt_len {
        bail!("delta target size mismatch: expected {tgt_len}, got {}", out.len());
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
            None => break,
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
            break;
        }
        let oid =
            raw_data[oid_start..oid_end].iter().fold(String::with_capacity(40), |mut s, b| {
                use std::fmt::Write;
                let _ = write!(s, "{b:02x}");
                s
            });

        entries.push(TreeEntry { mode, name, oid });
        i = oid_end;
    }

    Ok(GitTree { entries })
}

/// Parse `user.name` and `user.email` from a Git INI config string.
pub(crate) fn parse_git_user_config(text: &str) -> Option<(String, String)> {
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
