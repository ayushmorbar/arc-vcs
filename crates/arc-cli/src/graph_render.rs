use std::collections::HashSet;

use arc_core::algebra::Atom;
use arc_core::store::author::Author;
use arc_core::store::change::Change;
use arc_core::store::newtypes::ChangeId;
use owo_colors::OwoColorize;

/// Renders a revision DAG into stable, line-oriented ASCII/Unicode output.
#[derive(Debug, Clone, Copy)]
pub struct GraphRenderer {
    use_color: bool,
}

impl Default for GraphRenderer {
    fn default() -> Self {
        Self { use_color: true }
    }
}

impl GraphRenderer {
    /// Create a renderer that emits ANSI colors and semantic markers.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a renderer without ANSI colors.
    pub fn monochrome() -> Self {
        Self { use_color: false }
    }

    /// Render `changes` (expected newest-first) into graph lines.
    pub fn render(&self, changes: &[Change]) -> Vec<String> {
        let visible: HashSet<ChangeId> = changes.iter().map(|c| ChangeId::from(c.id)).collect();
        let mut active: Vec<ChangeId> = Vec::new();
        let mut lines = Vec::new();

        for change in changes {
            let id = ChangeId::from(change.id);
            let mut parents: Vec<ChangeId> = change
                .deps
                .iter()
                .copied()
                .map(ChangeId::from)
                .filter(|dep| visible.contains(dep))
                .collect();
            parents.sort();

            let col = if let Some(existing) = active.iter().position(|candidate| *candidate == id) {
                existing
            } else {
                active.insert(0, id);
                0
            };

            let symbol = self.node_symbol(change);
            let mut graph_prefix = String::new();
            for (idx, _) in active.iter().enumerate() {
                if idx > 0 {
                    graph_prefix.push(' ');
                }
                if idx == col {
                    graph_prefix.push_str(&symbol);
                } else {
                    graph_prefix.push('│');
                }
            }

            lines.push(format!(
                "{} {}",
                graph_prefix,
                self.row_label(change, change_is_ai(change), change_has_conflict(change))
            ));

            if parents.is_empty() {
                active.remove(col);
            } else {
                active[col] = parents[0];
                for (offset, parent) in parents.iter().enumerate().skip(1) {
                    if !active.contains(parent) {
                        active.insert(col + offset, *parent);
                    }
                }
            }

            dedupe_columns(&mut active);

            if parents.len() > 1 {
                lines.push(self.merge_edge_line(&active, col, parents.len()));
            }
        }

        lines
    }

    fn merge_edge_line(&self, active: &[ChangeId], from_col: usize, parent_count: usize) -> String {
        let to_col = from_col + parent_count.saturating_sub(1);
        let width = active.len().max(to_col + 1);
        let mut cells = String::new();

        for col in 0..width {
            if col > 0 {
                cells.push(' ');
            }
            if col < from_col {
                cells.push('│');
            } else if col == from_col {
                cells.push('├');
            } else if col == to_col {
                cells.push('╮');
            } else if col > from_col && col < to_col {
                cells.push('─');
            } else if col < active.len() {
                cells.push('│');
            } else {
                cells.push(' ');
            }
        }

        cells
    }

    fn node_symbol(&self, change: &Change) -> String {
        let is_ai = change_is_ai(change);
        let has_conflict = change_has_conflict(change);

        if !self.use_color {
            return if has_conflict || is_ai {
                "◉".to_string()
            } else {
                "○".to_string()
            };
        }

        if has_conflict {
            return "◉".red().bold().to_string();
        }
        if is_ai {
            return "◉".magenta().bold().to_string();
        }
        "○".cyan().to_string()
    }

    fn row_label(&self, change: &Change, is_ai: bool, has_conflict: bool) -> String {
        let short_id = ChangeId::from(change.id).to_hex()[..8].to_string();
        let author = author_label(&change.author);
        let mut tags = Vec::new();

        if has_conflict {
            tags.push(if self.use_color {
                "⚠".red().bold().to_string()
            } else {
                "⚠".to_string()
            });
        }
        if is_ai {
            tags.push(if self.use_color {
                "🤖".magenta().bold().to_string()
            } else {
                "🤖".to_string()
            });
        }

        if tags.is_empty() {
            format!("{} {} | {}", short_id, author, change.intent)
        } else {
            format!(
                "{} {} {} | {}",
                short_id,
                tags.join(""),
                author,
                change.intent
            )
        }
    }
}

fn dedupe_columns(active: &mut Vec<ChangeId>) {
    let mut seen = HashSet::new();
    active.retain(|id| seen.insert(*id));
}

fn change_has_conflict(change: &Change) -> bool {
    change
        .atoms
        .iter()
        .any(|atom| matches!(atom, Atom::Conflict { .. }))
}

fn change_is_ai(change: &Change) -> bool {
    matches!(change.author, Author::AI { .. })
}

fn author_label(author: &Author) -> String {
    match author {
        Author::Human { name, email, .. } => format!("{} <{}>", name, email),
        Author::AI {
            model,
            human_sponsor,
        } => {
            let sponsor: String = human_sponsor.iter().map(|b| format!("{:02x}", b)).collect();
            format!("{} sponsor:{}", model, &sponsor[..8])
        }
        Author::Server { canonical_id, .. } => format!("{} [server]", canonical_id),
        Author::Transient { session_id, .. } => format!("{} [transient]", session_id),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use arc_core::algebra::Atom;
    use arc_core::store::author::test_keypair;
    use arc_core::store::change::Change;

    use super::GraphRenderer;

    fn mk_change(deps: HashSet<[u8; 32]>, label: &str, with_conflict: bool, ai: bool) -> Change {
        let (author, key) = test_keypair();
        let author = if ai {
            match author {
                arc_core::store::author::Author::Human { key, .. } => {
                    arc_core::store::author::Author::AI {
                        model: "gpt-test".to_string(),
                        human_sponsor: key,
                    }
                }
                _ => unreachable!(),
            }
        } else {
            author
        };

        let atom = if with_conflict {
            Atom::Conflict {
                bases: vec![[1u8; 32]],
                sides: vec![[2u8; 32], [3u8; 32]],
                at: vec!["file".to_string(), "a.rs".to_string(), "fn_a".to_string()],
            }
        } else {
            Atom::Insert {
                at: vec![
                    "file".to_string(),
                    format!("{}.rs", label),
                    "fn_x".to_string(),
                ],
                content_hash: *blake3::hash(label.as_bytes()).as_bytes(),
            }
        };

        Change::new(deps, vec![atom], label, author, &key)
    }

    #[test]
    fn renders_merge_connectors() {
        let root = mk_change(HashSet::new(), "root", false, false);
        let left = mk_change(HashSet::from([root.id]), "left", false, false);
        let right = mk_change(HashSet::from([root.id]), "right", false, false);
        let merge = mk_change(HashSet::from([left.id, right.id]), "merge", false, false);

        let lines = GraphRenderer::monochrome().render(&[merge, right, left, root]);
        let has_merge_edge = lines
            .iter()
            .any(|line| line.contains("├") && line.contains("╮"));
        assert!(
            has_merge_edge,
            "merge output must include branch edge connectors"
        );
    }

    #[test]
    fn annotates_ai_and_conflict_rows() {
        let root = mk_change(HashSet::new(), "root", false, false);
        let ai = mk_change(HashSet::from([root.id]), "ai", false, true);
        let conflict = mk_change(HashSet::from([ai.id]), "conflict", true, false);

        let lines = GraphRenderer::monochrome().render(&[conflict, ai, root]);
        assert!(lines.iter().any(|line| line.contains("⚠")));
        assert!(lines.iter().any(|line| line.contains("🤖")));
    }
}
