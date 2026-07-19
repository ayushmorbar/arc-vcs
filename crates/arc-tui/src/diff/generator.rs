use std::collections::{HashMap, HashSet, VecDeque};

use arc_algebra::BlobStore;
use arc_algebra::apply::{MaterializedState, apply_change};
use arc_algebra_types::{Atom, Blake3Hash, NodePath};
use arc_change::Change;
use arc_store_types::author;
use ignore::gitignore::Gitignore;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
#[cfg(feature = "semantic-tree-sitter")]
use tree_sitter::Parser;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticKind {
    Insert,
    Delete,
    Modify,
    Unavailable,
}

#[derive(Debug, Clone)]
pub struct SemanticDiffLine {
    pub path: String,
    pub kind: SemanticKind,
    pub before: String,
    pub after: String,
}

#[derive(Debug, Clone)]
pub struct BinaryDiffMetadata {
    pub path: String,
    pub hash_hex: String,
    pub label: String,
}

#[derive(Debug, Clone)]
pub struct SemanticDiff {
    pub before: Text<'static>,
    pub after: Text<'static>,
    pub lines: Vec<SemanticDiffLine>,
    pub binary: Option<BinaryDiffMetadata>,
}

#[derive(Debug, Default, Clone)]
pub struct InMemoryBlobStore {
    blobs: HashMap<Blake3Hash, Vec<u8>>,
}

impl InMemoryBlobStore {
    pub fn insert_blob(&mut self, bytes: &[u8]) -> Blake3Hash {
        let hash = *blake3::hash(bytes).as_bytes();
        self.blobs.insert(hash, bytes.to_vec());
        hash
    }
}

impl BlobStore for InMemoryBlobStore {
    fn read_blob(&self, hash: &Blake3Hash) -> Result<Vec<u8>, String> {
        self.blobs.get(hash).cloned().ok_or_else(|| format!("blob not found: {hash:?}"))
    }

    fn contains_blob(&self, hash: &Blake3Hash) -> bool {
        self.blobs.contains_key(hash)
    }
}

pub struct DiffGenerator<'a, S: BlobStore> {
    store: &'a S,
    #[cfg(feature = "semantic-tree-sitter")]
    parser: Parser,
}

impl<'a, S: BlobStore> DiffGenerator<'a, S> {
    pub fn new(store: &'a S) -> Self {
        #[cfg(feature = "semantic-tree-sitter")]
        {
            let mut parser = Parser::new();
            let _ = parser.set_language(&tree_sitter_rust::LANGUAGE.into());
            return Self { store, parser };
        }

        #[cfg(not(feature = "semantic-tree-sitter"))]
        {
            Self { store }
        }
    }

    pub fn generate(&mut self, change: &Change) -> Result<SemanticDiff, String> {
        let before_change = self.synthetic_before_change(change)?;

        let mut before_state = MaterializedState::new();
        apply_change(&mut before_state, &before_change, self.store, &Gitignore::empty(), None)?;

        let mut after_state = before_state.clone();
        apply_change(&mut after_state, change, self.store, &Gitignore::empty(), None)?;

        let mut before_lines = Vec::new();
        let mut after_lines = Vec::new();
        let mut lines = Vec::new();
        let mut binary = None;
        let mut paired_deletes = self.paired_deletes(change);

        for atom in &change.atoms {
            match atom {
                Atom::Insert { at, content_hash } => {
                    let path = node_path_to_string(at);
                    let after_text = self.decode_hash(content_hash);
                    let (before_text, kind) = if let Some(hashes) = paired_deletes.get_mut(&path) {
                        if let Some(prior_hash) = hashes.pop_front() {
                            (self.decode_hash(&prior_hash), SemanticKind::Modify)
                        } else {
                            let before = before_state
                                .get(at)
                                .map(|v| String::from_utf8_lossy(v).to_string())
                                .unwrap_or_default();
                            let kind = if before_state.contains_key(at) {
                                SemanticKind::Modify
                            } else {
                                SemanticKind::Insert
                            };
                            (before, kind)
                        }
                    } else {
                        let before = before_state
                            .get(at)
                            .map(|v| String::from_utf8_lossy(v).to_string())
                            .unwrap_or_default();
                        let kind = if before_state.contains_key(at) {
                            SemanticKind::Modify
                        } else {
                            SemanticKind::Insert
                        };
                        (before, kind)
                    };

                    before_lines.push(self.color_line(&path, &before_text, kind));
                    after_lines.push(self.color_line(&path, &after_text, kind));
                    lines.push(SemanticDiffLine {
                        path,
                        kind,
                        before: before_text,
                        after: after_text,
                    });
                }
                Atom::Delete { at, prior_hash } => {
                    let path = node_path_to_string(at);

                    let before_text = self.decode_hash(prior_hash);
                    let after_text = String::new();
                    let kind = SemanticKind::Delete;

                    before_lines.push(self.color_line(&path, &before_text, kind));
                    after_lines.push(self.color_line(&path, &after_text, kind));
                    lines.push(SemanticDiffLine {
                        path,
                        kind,
                        before: before_text,
                        after: after_text,
                    });
                }
                Atom::Blob { path, hash, .. } => {
                    if is_binary_path(path) {
                        binary = Some(BinaryDiffMetadata {
                            path: path.clone(),
                            hash_hex: hash.to_hex().to_string(),
                            label: "Binary Change".to_string(),
                        });
                    }
                }
                _ => {
                    let unsupported = describe_atom(atom);
                    let label = format!("semantic-unavailable: {unsupported}");
                    before_lines.push(self.color_line(
                        "[semantic]",
                        &label,
                        SemanticKind::Unavailable,
                    ));
                    after_lines.push(self.color_line(
                        "[semantic]",
                        &label,
                        SemanticKind::Unavailable,
                    ));
                    lines.push(SemanticDiffLine {
                        path: "[semantic]".to_string(),
                        kind: SemanticKind::Unavailable,
                        before: label.clone(),
                        after: label,
                    });
                }
            }
        }

        Ok(SemanticDiff {
            before: Text::from(before_lines),
            after: Text::from(after_lines),
            lines,
            binary,
        })
    }

    fn paired_deletes(&self, change: &Change) -> HashMap<String, VecDeque<Blake3Hash>> {
        let mut delete_hashes: HashMap<String, Vec<Blake3Hash>> = HashMap::new();
        let mut insert_counts: HashMap<String, usize> = HashMap::new();

        for atom in &change.atoms {
            match atom {
                Atom::Delete { at, prior_hash } => {
                    delete_hashes.entry(node_path_to_string(at)).or_default().push(*prior_hash);
                }
                Atom::Insert { at, .. } => {
                    let path = node_path_to_string(at);
                    insert_counts.entry(path).and_modify(|count| *count += 1).or_insert(1);
                }
                _ => {}
            }
        }

        delete_hashes
            .into_iter()
            .filter_map(|(path, hashes)| {
                let paired = insert_counts.get(&path).copied().unwrap_or(0).min(hashes.len());
                if paired == 0 {
                    return None;
                }
                Some((path, hashes.into_iter().take(paired).collect::<VecDeque<_>>()))
            })
            .collect()
    }

    fn synthetic_before_change(&self, change: &Change) -> Result<Change, String> {
        let mut atoms = Vec::new();
        for atom in &change.atoms {
            if let Atom::Delete { at, prior_hash } = atom {
                atoms.push(Atom::Insert { at: at.clone(), content_hash: *prior_hash });
            }
        }

        let (author, signing_key) = author::test_keypair();
        Ok(Change::new(HashSet::new(), atoms, "diff-before-state", author, &signing_key))
    }

    fn decode_hash(&self, hash: &Blake3Hash) -> String {
        match self.store.read_blob(hash) {
            Ok(bytes) => String::from_utf8_lossy(&bytes).to_string(),
            Err(_) => "<missing blob>".to_string(),
        }
    }

    fn color_line(&mut self, path: &str, value: &str, kind: SemanticKind) -> Line<'static> {
        #[cfg(feature = "semantic-tree-sitter")]
        {
            let _ = self.parser.parse(value, None);
        }

        let style = match kind {
            SemanticKind::Insert => Style::default().fg(Color::Green),
            SemanticKind::Delete => Style::default().fg(Color::Red),
            SemanticKind::Modify => Style::default().fg(Color::Cyan),
            SemanticKind::Unavailable => Style::default().fg(Color::Yellow),
        };
        let label = format!("{path}: ");
        Line::from(vec![Span::styled(label, style), Span::styled(value.to_string(), style)])
    }
}

fn node_path_to_string(path: &NodePath) -> String {
    path.join("/")
}

pub fn is_binary_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".png")
        || lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
        || lower.ends_with(".gif")
        || lower.ends_with(".webp")
}

fn describe_atom(atom: &Atom) -> &'static str {
    match atom {
        Atom::Move { .. } => "move",
        Atom::SemanticsPreserving { .. } => "semantics-preserving",
        Atom::Directory { .. } => "directory",
        Atom::Blob { .. } => "blob",
        Atom::Insert { .. } => "insert",
        Atom::Delete { .. } => "delete",
        Atom::Mount { .. } => "mount",
        Atom::Conflict { .. } => "conflict",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use arc_algebra_types::Atom;
    use arc_change::Change;
    use arc_store_types::author;

    use super::{DiffGenerator, InMemoryBlobStore, SemanticKind};

    #[test]
    fn classify_insert_delete_modify_colors() {
        let mut store = InMemoryBlobStore::default();
        let ins = store.insert_blob(b"fn new_fn() {}\n");
        let old = store.insert_blob(b"let x = 1;\n");
        let new = store.insert_blob(b"let x = 2;\n");

        let (author, signing_key) = author::test_keypair();
        let change = Change::new(
            HashSet::new(),
            vec![
                Atom::Insert {
                    at: vec!["file".into(), "src/lib.rs".into(), "new_fn".into()],
                    content_hash: ins,
                },
                Atom::Delete {
                    at: vec!["file".into(), "src/lib.rs".into(), "old_line".into()],
                    prior_hash: old,
                },
                Atom::Insert {
                    at: vec!["file".into(), "src/lib.rs".into(), "old_line".into()],
                    content_hash: new,
                },
            ],
            "mock semantic diff",
            author,
            &signing_key,
        );

        let mut generator = DiffGenerator::new(&store);
        let diff = generator.generate(&change).expect("generate diff");

        assert!(diff.lines.iter().any(|line| line.kind == SemanticKind::Insert));
        assert!(diff.lines.iter().any(|line| line.kind == SemanticKind::Delete));
        assert!(diff.lines.iter().any(|line| line.kind == SemanticKind::Modify));
    }
}
