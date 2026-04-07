use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use arc_core::store::oplog::{Causality, OpAction, OpRecord, auto_intent_summary};
use arc_core::store::redb_metadata::MetadataStore;
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use tree_sitter::{Node, Parser};
use tokio::sync::mpsc;
use tokio::time::timeout;

#[derive(Default)]
struct TransientBuffer {
    paths: BTreeSet<PathBuf>,
}

impl TransientBuffer {
    fn replace(&mut self, paths: BTreeSet<PathBuf>) {
        self.paths = paths;
    }

    fn clear(&mut self) {
        self.paths.clear();
    }
}

#[derive(Default)]
struct SemanticArtifacts {
    has_parse_errors: bool,
    symbols: Vec<String>,
}

/// Background autosnapshot daemon.
pub struct AutoSnapDaemon;

impl AutoSnapDaemon {
    /// Start recursive watch + debounced autosnapshot loop.
    pub async fn start(path: PathBuf, debounce_ms: u64) -> anyhow::Result<()> {
        let debounce = Duration::from_millis(debounce_ms.max(1));
        let (_watcher, mut rx) = start_watcher(&path)?;
        let metadata_store = MetadataStore::open(&path)
            .with_context(|| format!("failed to open metadata store at {}", path.display()))?;

        let mut pending_paths = BTreeSet::new();
        let mut transient = TransientBuffer::default();

        tracing::info!(
            path = %path.display(),
            debounce_ms,
            "[arc-watch] watcher started"
        );

        while let Some(initial_event) = rx.recv().await {
            collect_event_paths(&mut pending_paths, &initial_event);
            loop {
                match timeout(debounce, rx.recv()).await {
                    Ok(Some(event)) => {
                        collect_event_paths(&mut pending_paths, &event);
                        // Another event arrived inside the debounce window; reset timer.
                        continue;
                    }
                    Ok(None) => return Ok(()),
                    Err(_) => {
                        pending_paths.extend(transient.paths.iter().cloned());
                        let artifacts = evaluate_semantic_gate(&pending_paths)?;
                        if artifacts.has_parse_errors {
                            transient.replace(pending_paths.clone());
                            tracing::info!(
                                transient_count = transient.paths.len(),
                                "[arc-watch] Semantic Gate blocked snap; buffered transient changes"
                            );
                            pending_paths.clear();
                            break;
                        }

                        let target_oid = snapshot_oid_for_paths(&pending_paths);
                        let snap_id = format!(
                            "snap-{}-{}",
                            unix_timestamp_nanos(),
                            hex::encode(&target_oid[..4])
                        );
                        let intent_summary = Some(auto_intent_summary(&artifacts.symbols));
                        let record = OpRecord {
                            id: snap_id,
                            action: OpAction::Snap,
                            causality: Causality::Local,
                            timestamp: unix_timestamp_secs(),
                            target_oid,
                            intent_summary,
                        };

                        metadata_store.put_op_record(&record).context(
                            "failed to persist semantic snap metadata record",
                        )?;

                        transient.clear();
                        pending_paths.clear();
                        tracing::info!(
                            op_id = %record.id,
                            "[arc-watch] Debounce fired. Semantic Gate passed; flushed structured Snap"
                        );
                        break;
                    }
                }
            }
        }

        Ok(())
    }
}

fn start_watcher(path: &Path) -> anyhow::Result<(RecommendedWatcher, mpsc::Receiver<Event>)> {
    let (tx, rx) = mpsc::channel::<Event>(1024);
    let callback_tx = tx.clone();

    let mut watcher = RecommendedWatcher::new(
        move |event: notify::Result<Event>| match event {
            Ok(event) => {
                // If the channel is saturated, dropping event bursts is fine:
                // debounce only needs a signal that "something changed".
                let _ = callback_tx.try_send(event);
            }
            Err(err) => {
                tracing::warn!(error = %err, "[arc-watch] filesystem watcher error");
            }
        },
        Config::default(),
    )
    .context("failed to create notify watcher")?;

    watcher
        .watch(path, RecursiveMode::Recursive)
        .with_context(|| format!("failed to watch {}", path.display()))?;

    Ok((watcher, rx))
}

fn collect_event_paths(pending_paths: &mut BTreeSet<PathBuf>, event: &Event) {
    for path in &event.paths {
        pending_paths.insert(path.to_path_buf());
    }
}

fn evaluate_semantic_gate(paths: &BTreeSet<PathBuf>) -> anyhow::Result<SemanticArtifacts> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .context("failed to set Rust grammar for semantic gate")?;

    let mut artifacts = SemanticArtifacts::default();

    for path in paths {
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }

        let buffer = match std::fs::read_to_string(path) {
            Ok(content) => content,
            Err(err) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %err,
                    "[arc-watch] semantic gate skipped unreadable buffer"
                );
                continue;
            }
        };
        let Some(tree) = parser.parse(&buffer, None) else {
            artifacts.has_parse_errors = true;
            continue;
        };

        if has_error_nodes(tree.root_node()) {
            artifacts.has_parse_errors = true;
            continue;
        }

        artifacts.symbols.extend(extract_symbols(&buffer, tree.root_node()));
    }

    Ok(artifacts)
}

fn has_error_nodes(node: Node<'_>) -> bool {
    if node.is_error() || node.is_missing() {
        return true;
    }
    let mut cursor = node.walk();
    node.children(&mut cursor).any(has_error_nodes)
}

fn extract_symbols(source: &str, root: Node<'_>) -> Vec<String> {
    let bytes = source.as_bytes();
    let mut symbols = Vec::new();
    collect_symbols_recursive(root, bytes, &mut symbols);
    symbols.sort();
    symbols.dedup();
    symbols
}

fn collect_symbols_recursive(node: Node<'_>, bytes: &[u8], symbols: &mut Vec<String>) {
    match node.kind() {
        "function_item" | "struct_item" | "enum_item" | "trait_item" | "type_item" => {
            if let Some(name_node) = node.child_by_field_name("name")
                && let Ok(name) = name_node.utf8_text(bytes)
            {
                symbols.push(name.to_string());
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_symbols_recursive(child, bytes, symbols);
    }
}

fn unix_timestamp_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

    fn unix_timestamp_nanos() -> u128 {
        SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
    }

fn snapshot_oid_for_paths(paths: &BTreeSet<PathBuf>) -> [u8; 20] {
    let mut hasher = blake3::Hasher::new();
    for path in paths {
        hasher.update(path.to_string_lossy().as_bytes());
        match std::fs::read(path) {
            Ok(bytes) => hasher.update(&bytes),
            Err(_) => hasher.update(b"<missing-or-unreadable>"),
        };
    }

    let digest = hasher.finalize();
    let mut oid = [0u8; 20];
    oid.copy_from_slice(&digest.as_bytes()[..20]);
    oid
}

#[cfg(test)]
mod tests {
    use super::{evaluate_semantic_gate, has_error_nodes};
    use std::collections::BTreeSet;
    use std::io::Write;

    #[test]
    fn semantic_gate_detects_parse_errors() {
        let dir = tempfile::tempdir().expect("tempdir should be created");
        let path = dir.path().join("bad.rs");
        std::fs::write(&path, "fn broken( {").expect("write should succeed");

        let mut paths = BTreeSet::new();
        paths.insert(path);

        let artifacts = evaluate_semantic_gate(&paths).expect("gate evaluation should run");
        assert!(artifacts.has_parse_errors);
    }

    #[test]
    fn semantic_gate_extracts_symbols_from_valid_rust() {
        let dir = tempfile::tempdir().expect("tempdir should be created");
        let path = dir.path().join("ok.rs");
        let mut file = std::fs::File::create(&path).expect("file must be creatable");
        writeln!(file, "fn compute_total() {{}}\nstruct Invoice;")
            .expect("write should succeed");

        let mut paths = BTreeSet::new();
        paths.insert(path);

        let artifacts = evaluate_semantic_gate(&paths).expect("gate evaluation should run");
        assert!(!artifacts.has_parse_errors);
        assert!(artifacts.symbols.iter().any(|s| s == "compute_total"));
        assert!(artifacts.symbols.iter().any(|s| s == "Invoice"));
    }

    #[test]
    fn error_node_helper_flags_invalid_tree() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .expect("language should set");
        let tree = parser.parse("fn broken(", None).expect("parse should return a tree");
        assert!(has_error_nodes(tree.root_node()));
    }
}
