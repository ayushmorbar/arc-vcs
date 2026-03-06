use std::collections::{HashSet, VecDeque};

use crate::algebra::Blake3Hash;
use crate::store::change::Change;
use crate::store::repo::Repository;
use crate::store::view::View;

/// Fetch missing changes from a remote repository's view into the local
/// repository.
///
/// `remote_path` is either a local filesystem path **or** an `http://` /
/// `https://` URL pointing at an `arc serve` instance.
///
/// The Bounded BFS algorithm is identical in both cases: any change already
/// present in the local store is a causal cut-point — its ancestors are
/// guaranteed to be present locally, so they are not enqueued.
///
/// Returns the remote view's heads.
pub fn fetch(
    local: &mut Repository,
    remote_path: &str,
    view_name: &str,
) -> anyhow::Result<HashSet<Blake3Hash>> {
    if remote_path.starts_with("http://") || remote_path.starts_with("https://") {
        return fetch_http(local, remote_path, view_name);
    }
    fetch_local(local, remote_path, view_name)
}

fn fetch_local(
    local: &mut Repository,
    remote_path: &str,
    view_name: &str,
) -> anyhow::Result<HashSet<Blake3Hash>> {
    let remote = Repository::open(remote_path)?;
    let remote_view = View::load(&remote.root, view_name)
        .map_err(|e| anyhow::anyhow!("failed to load remote view '{view_name}': {e}"))?;

    let mut queue: VecDeque<Blake3Hash> = remote_view.heads.iter().copied().collect();
    let mut visited = HashSet::new();

    while let Some(id) = queue.pop_front() {
        if !visited.insert(id) {
            continue;
        }

        // Bounded BFS: if the local store already has this change,
        // all its ancestors are causally guaranteed to be present.
        if local.store.read_change(&id).is_ok() {
            if local.graph.get(&id).is_none() {
                let change = local.store.read_change(&id).unwrap();
                local.graph.add_change(change);
            }
            continue;
        }

        let change = remote
            .store
            .read_change(&id)
            .map_err(|e| anyhow::anyhow!("failed to read change from remote CAS: {e}"))?;
        local
            .store
            .write_change(&change)
            .map_err(|e| anyhow::anyhow!("failed to write change to local CAS: {e}"))?;
        for &dep in &change.deps {
            if !visited.contains(&dep) {
                queue.push_back(dep);
            }
        }
        local.graph.add_change(change);
    }

    Ok(remote_view.heads)
}

fn fetch_http(
    local: &mut Repository,
    remote_url: &str,
    view_name: &str,
) -> anyhow::Result<HashSet<Blake3Hash>> {
    let url = format!("{remote_url}/views/{view_name}");
    let remote_view: View = reqwest::blocking::get(&url)
        .map_err(|e| anyhow::anyhow!("HTTP GET {url} failed: {e}"))?
        .error_for_status()
        .map_err(|e| anyhow::anyhow!("server returned error for {url}: {e}"))?
        .json::<View>()
        .map_err(|e| anyhow::anyhow!("failed to deserialise remote view: {e}"))?;

    let mut queue: VecDeque<Blake3Hash> = remote_view.heads.iter().copied().collect();
    let mut visited: HashSet<Blake3Hash> = HashSet::new();
    let client = reqwest::blocking::Client::new();

    while let Some(id) = queue.pop_front() {
        if !visited.insert(id) {
            continue;
        }

        if local.store.read_change(&id).is_ok() {
            if local.graph.get(&id).is_none() {
                let change = local.store.read_change(&id).unwrap();
                local.graph.add_change(change);
            }
            continue;
        }

        let hex: String = id.iter().map(|b| format!("{b:02x}")).collect();
        let obj_url = format!("{remote_url}/objects/{hex}");
        let bytes = client
            .get(&obj_url)
            .send()
            .map_err(|e| anyhow::anyhow!("HTTP GET {obj_url} failed: {e}"))?
            .error_for_status()
            .map_err(|e| anyhow::anyhow!("server returned error for {obj_url}: {e}"))?
            .bytes()
            .map_err(|e| anyhow::anyhow!("failed to read object bytes: {e}"))?;
        let change: Change = bincode::deserialize(&bytes)
            .map_err(|e| anyhow::anyhow!("failed to deserialise change {hex}: {e}"))?;
        local
            .store
            .write_change(&change)
            .map_err(|e| anyhow::anyhow!("failed to write change {hex} to local CAS: {e}"))?;
        for &dep in &change.deps {
            if !visited.contains(&dep) {
                queue.push_back(dep);
            }
        }
        local.graph.add_change(change);
    }

    Ok(remote_view.heads)
}

/// Pull changes from a remote repository's view and merge them into the
/// local active view.
///
/// This is `fetch` followed by `merge_heads` — the CRDT sync primitive.
pub fn pull(
    local: &mut Repository,
    remote_path: &str,
    view_name: &str,
) -> anyhow::Result<()> {
    let remote_heads = fetch(local, remote_path, view_name)?;
    local.merge_heads(&remote_heads)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn test_network_pull() {
        let dir = tempfile::tempdir().unwrap();
        let path_a = dir.path().join("repo_a");
        let path_b = dir.path().join("repo_b");

        // --- Set up Repo A with a file ---
        let mut repo_a = Repository::init(&path_a).unwrap();
        let (author_a, key_a) = crate::store::author::test_keypair();
        repo_a.set_identity(author_a, key_a);
        fs::write(path_a.join("a.rs"), "fn a() {}").unwrap();
        repo_a.snap("add a.rs").unwrap();

        // --- Init Repo B and pull from A ---
        let mut repo_b = Repository::init(&path_b).unwrap();
        let (author_b, key_b) = crate::store::author::test_keypair();
        repo_b.set_identity(author_b, key_b);
        pull(&mut repo_b, path_a.to_str().unwrap(), "main").unwrap();

        // B should now have a.rs on disk.
        assert!(
            path_b.join("a.rs").exists(),
            "a.rs must exist in repo B after pull"
        );
        assert_eq!(
            fs::read_to_string(path_b.join("a.rs")).unwrap(),
            "fn a() {}"
        );

        // --- Diverge: A adds c.rs, B adds b.rs ---
        fs::write(path_a.join("c.rs"), "fn c() {}").unwrap();
        repo_a.snap("add c.rs").unwrap();

        fs::write(path_b.join("b.rs"), "fn b() {}").unwrap();
        repo_b.snap("add b.rs").unwrap();

        // --- Pull A into B — disjoint files must commute ---
        pull(&mut repo_b, path_a.to_str().unwrap(), "main").unwrap();

        // B's working directory should have all three files.
        assert!(
            path_b.join("a.rs").exists(),
            "a.rs must survive the merge"
        );
        assert!(
            path_b.join("b.rs").exists(),
            "b.rs (local to B) must survive the merge"
        );
        assert!(
            path_b.join("c.rs").exists(),
            "c.rs (from A) must appear after pull"
        );

        // Verify graph completeness: B's graph should have all of A's changes.
        let view_b = View::load(&path_b, "main").unwrap();
        assert!(
            view_b.heads.len() >= 2,
            "merged view must have at least 2 heads after divergent pull, got: {}",
            view_b.heads.len()
        );

        // Verify content.
        assert_eq!(fs::read_to_string(path_b.join("a.rs")).unwrap(), "fn a() {}");
        assert_eq!(fs::read_to_string(path_b.join("b.rs")).unwrap(), "fn b() {}");
        assert_eq!(fs::read_to_string(path_b.join("c.rs")).unwrap(), "fn c() {}");
    }
}

