use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use serde::Deserialize;
use serde_json::Value;

use crate::Blake3Hash;
use crate::newtypes::ChangeId;
use crate::tag::Tag;

#[derive(Debug, Deserialize)]
struct GenericRefFile {
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    head: Option<String>,
    #[serde(default)]
    hash: Option<String>,
    #[serde(default)]
    change_id: Option<String>,
    #[serde(default)]
    heads: Vec<String>,
}

/// Resolve all tag targets to a strongly-typed set of change IDs.
pub fn read_tag_heads(shared_root: &Path) -> anyhow::Result<BTreeSet<ChangeId>> {
    Ok(read_tag_map(shared_root)?.keys().copied().collect())
}

/// Resolve all remote-tracking branch heads to a strongly-typed set.
pub fn read_remote_branch_heads(shared_root: &Path) -> anyhow::Result<BTreeSet<ChangeId>> {
    Ok(read_remote_branch_map(shared_root)?.keys().copied().collect())
}

/// Resolve all bookmark heads to a strongly-typed set.
pub fn read_bookmark_heads(shared_root: &Path) -> anyhow::Result<BTreeSet<ChangeId>> {
    Ok(read_bookmark_map(shared_root)?.keys().copied().collect())
}

/// Read tags as a map of target change id -> tag names.
pub fn read_tag_map(shared_root: &Path) -> anyhow::Result<BTreeMap<ChangeId, Vec<String>>> {
    let mut out: BTreeMap<ChangeId, Vec<String>> = BTreeMap::new();

    for base in
        [shared_root.join(".arc").join("refs").join("tags"), shared_root.join(".arc").join("tags")]
    {
        if !base.exists() {
            continue;
        }
        for path in gather_files(&base)? {
            let bytes = fs::read(&path)
                .with_context(|| format!("failed to read tag file {}", path.display()))?;
            if let Some((name, id)) = parse_tag_record(&path, &bytes) {
                out.entry(id).or_default().push(name);
            }
        }
    }

    for names in out.values_mut() {
        names.sort();
        names.dedup();
    }

    Ok(out)
}

/// Read remote-tracking branches as a map of target change id -> remote ref names.
pub fn read_remote_branch_map(
    shared_root: &Path,
) -> anyhow::Result<BTreeMap<ChangeId, Vec<String>>> {
    let mut out: BTreeMap<ChangeId, Vec<String>> = BTreeMap::new();
    let base = shared_root.join(".arc").join("refs").join("remotes");

    if !base.exists() {
        return Ok(out);
    }

    for path in gather_files(&base)? {
        let bytes = fs::read(&path)
            .with_context(|| format!("failed to read remote ref file {}", path.display()))?;
        let ref_name = normalize_ref_name(&base, &path);
        for id in parse_reference_targets(&bytes) {
            out.entry(id).or_default().push(ref_name.clone());
        }
    }

    for names in out.values_mut() {
        names.sort();
        names.dedup();
    }

    Ok(out)
}

/// Read bookmarks as a map of target change id -> bookmark names.
pub fn read_bookmark_map(shared_root: &Path) -> anyhow::Result<BTreeMap<ChangeId, Vec<String>>> {
    let mut out: BTreeMap<ChangeId, Vec<String>> = BTreeMap::new();
    let base = shared_root.join(".arc").join("refs").join("bookmarks");

    if !base.exists() {
        return Ok(out);
    }

    for path in gather_files(&base)? {
        let bytes = fs::read(&path)
            .with_context(|| format!("failed to read bookmark ref file {}", path.display()))?;
        let ref_name = normalize_ref_name(&base, &path);
        for id in parse_reference_targets(&bytes) {
            out.entry(id).or_default().push(ref_name.clone());
        }
    }

    for names in out.values_mut() {
        names.sort();
        names.dedup();
    }

    Ok(out)
}

fn gather_files(root: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    if !root.exists() {
        return Ok(files);
    }

    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)
            .with_context(|| format!("failed to read directory {}", dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() {
                files.push(path);
            }
        }
    }

    files.sort();
    Ok(files)
}

fn parse_tag_record(path: &Path, bytes: &[u8]) -> Option<(String, ChangeId)> {
    if let Ok(tag) = serde_json::from_slice::<Tag>(bytes) {
        return Some((tag.name, ChangeId::from(tag.target)));
    }

    parse_reference_targets(bytes).into_iter().next().map(|id| (file_stem_or_name(path), id))
}

fn file_stem_or_name(path: &Path) -> String {
    path.file_stem()
        .or_else(|| path.file_name())
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "ref".to_string())
}

fn normalize_ref_name(base: &Path, path: &Path) -> String {
    path.strip_prefix(base)
        .ok()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|| file_stem_or_name(path))
}

fn parse_reference_targets(bytes: &[u8]) -> Vec<ChangeId> {
    if let Ok(text) = std::str::from_utf8(bytes) {
        let mut from_text = Vec::new();
        for token in text.split_whitespace() {
            if let Ok(id) = ChangeId::from_hex(token) {
                from_text.push(id);
            }
        }
        if !from_text.is_empty() {
            return from_text;
        }
    }

    if let Ok(id) = serde_json::from_slice::<Blake3Hash>(bytes) {
        return vec![ChangeId::from(id)];
    }

    if let Ok(hex) = serde_json::from_slice::<String>(bytes)
        && let Ok(id) = ChangeId::from_hex(hex.trim())
    {
        return vec![id];
    }

    if let Ok(raw) = serde_json::from_slice::<GenericRefFile>(bytes) {
        let mut out = Vec::new();
        for candidate in [raw.target, raw.head, raw.hash, raw.change_id].into_iter().flatten() {
            if let Ok(id) = ChangeId::from_hex(candidate.trim()) {
                out.push(id);
            }
        }
        for head in raw.heads {
            if let Ok(id) = ChangeId::from_hex(head.trim()) {
                out.push(id);
            }
        }
        if !out.is_empty() {
            out.sort();
            out.dedup();
            return out;
        }
    }

    if let Ok(value) = serde_json::from_slice::<Value>(bytes) {
        let mut out = Vec::new();
        collect_hashes_from_json(&value, &mut out);
        if !out.is_empty() {
            out.sort();
            out.dedup();
            return out;
        }
    }

    Vec::new()
}

fn collect_hashes_from_json(value: &Value, out: &mut Vec<ChangeId>) {
    match value {
        Value::String(s) => {
            if let Ok(id) = ChangeId::from_hex(s.trim()) {
                out.push(id);
            }
        }
        Value::Array(arr) => {
            if arr.len() == 32 && arr.iter().all(|item| item.as_u64().is_some()) {
                let mut hash = [0u8; 32];
                for (idx, item) in arr.iter().enumerate() {
                    hash[idx] = item.as_u64().unwrap_or_default() as u8;
                }
                out.push(ChangeId::from(hash));
                return;
            }
            for item in arr {
                collect_hashes_from_json(item, out);
            }
        }
        Value::Object(map) => {
            for key in ["target", "head", "hash", "change_id", "heads"] {
                if let Some(v) = map.get(key) {
                    collect_hashes_from_json(v, out);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use crate::author::test_keypair;

    use super::*;

    #[test]
    fn reads_tag_heads_from_arc_tags_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let tag_dir = root.join(".arc").join("tags");
        fs::create_dir_all(&tag_dir).expect("create tag dir");

        let (author, key) = test_keypair();
        let target = [7u8; 32];
        let tag = Tag::new("v1.0.0", target, author, &key);
        fs::write(
            tag_dir.join("v1.0.0.json"),
            serde_json::to_vec_pretty(&tag).expect("serialize tag"),
        )
        .expect("write tag");

        let heads = read_tag_heads(root).expect("read tag heads");
        assert_eq!(heads, BTreeSet::from([ChangeId::from(target)]));
    }

    #[test]
    fn reads_remote_heads_from_refs_namespace() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let refs_dir = root.join(".arc").join("refs").join("remotes");
        fs::create_dir_all(refs_dir.join("origin")).expect("create refs dir");

        let head = [3u8; 32];
        let hex = ChangeId::from(head).to_hex();
        fs::write(refs_dir.join("origin").join("main"), hex).expect("write remote ref");

        let heads = read_remote_branch_heads(root).expect("read remote heads");
        assert_eq!(heads, BTreeSet::from([ChangeId::from(head)]));
    }

    #[test]
    fn reads_bookmark_heads_from_refs_namespace() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let refs_dir = root.join(".arc").join("refs").join("bookmarks");
        fs::create_dir_all(refs_dir.join("feature")).expect("create bookmark refs dir");

        let head = [11u8; 32];
        let hex = ChangeId::from(head).to_hex();
        fs::write(refs_dir.join("feature").join("ui"), hex).expect("write bookmark ref");

        let heads = read_bookmark_heads(root).expect("read bookmark heads");
        assert_eq!(heads, BTreeSet::from([ChangeId::from(head)]));
    }
}
