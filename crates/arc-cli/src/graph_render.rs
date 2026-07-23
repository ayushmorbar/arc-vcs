use std::collections::{BTreeMap, BTreeSet, HashSet};

use arc_algebra_types::Atom;
use arc_change::Change;
use arc_store_types::{author::Author, newtypes::ChangeId};
use owo_colors::OwoColorize;

/// Renders a revision DAG into stable, line-oriented ASCII/Unicode output.
#[derive(Debug, Clone, Copy)]
pub struct GraphRenderer {
    use_color: bool,
}

/// Decorations keyed by commit id for renderer row labels.
#[derive(Debug, Clone, Default)]
pub struct GraphDecorations {
    /// Tag names by target change id.
    pub tags: BTreeMap<ChangeId, Vec<String>>,
    /// Remote-tracking branch names by tracked head id.
    pub remotes: BTreeMap<ChangeId, Vec<String>>,
    /// Current checkout change id (rendered as `@`).
    pub current: Option<ChangeId>,
    /// Active local heads in the current view.
    pub active_heads: BTreeSet<ChangeId>,
    /// Stable ancestors that should be visually de-emphasized.
    pub stable_ancestors: BTreeSet<ChangeId>,
}

/// Parsed user template for `arc log` row labels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogTemplate {
    parts: Vec<TemplatePart>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TemplatePart {
    Literal(String),
    Field(TemplateField),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TemplateField {
    Id,
    IdShort,
    Author,
    Intent,
    StateBadges,
    RefBadges,
    Badges,
}

#[derive(Debug, Clone, Copy)]
struct TemplateContext<'a> {
    id: &'a str,
    id_short: &'a str,
    author: &'a str,
    intent: &'a str,
    state_badges: &'a str,
    ref_badges: &'a str,
    badges: &'a str,
}

#[derive(Debug, Clone, Copy)]
struct RowFlags {
    is_ai: bool,
    has_conflict: bool,
    is_current: bool,
    is_active_head: bool,
}

impl LogTemplate {
    /// Parse a KISS placeholder template (e.g. `{id_short} {author} | {intent}`).
    pub fn parse(input: &str) -> Result<Self, String> {
        let mut parts = Vec::new();
        let mut literal = String::new();
        let mut chars = input.chars();

        while let Some(ch) = chars.next() {
            if ch == '{' {
                if !literal.is_empty() {
                    parts.push(TemplatePart::Literal(std::mem::take(&mut literal)));
                }
                let mut field_name = String::new();
                let mut closed = false;
                for c in chars.by_ref() {
                    if c == '}' {
                        closed = true;
                        break;
                    }
                    field_name.push(c);
                }
                if !closed {
                    return Err("template contains an unclosed '{'".to_string());
                }

                let field = TemplateField::parse(field_name.trim())?;
                parts.push(TemplatePart::Field(field));
            } else if ch == '}' {
                return Err("template contains an unmatched '}'".to_string());
            } else {
                literal.push(ch);
            }
        }

        if !literal.is_empty() {
            parts.push(TemplatePart::Literal(literal));
        }

        Ok(Self { parts })
    }

    fn render(&self, ctx: TemplateContext<'_>) -> String {
        let mut rendered = String::new();
        for part in &self.parts {
            match part {
                TemplatePart::Literal(text) => rendered.push_str(text),
                TemplatePart::Field(field) => rendered.push_str(field.render(ctx)),
            }
        }
        rendered
    }
}

impl TemplateField {
    fn parse(name: &str) -> Result<Self, String> {
        match name {
            "id" => Ok(Self::Id),
            "id_short" => Ok(Self::IdShort),
            "author" => Ok(Self::Author),
            "intent" => Ok(Self::Intent),
            "state_badges" => Ok(Self::StateBadges),
            "ref_badges" => Ok(Self::RefBadges),
            "badges" => Ok(Self::Badges),
            "" => Err("template contains an empty placeholder '{}'".to_string()),
            _ => Err(format!(
                "unsupported template field '{name}'. Supported fields: id, id_short, author, \
                 intent, state_badges, ref_badges, badges"
            )),
        }
    }

    fn render(self, ctx: TemplateContext<'_>) -> &str {
        match self {
            Self::Id => ctx.id,
            Self::IdShort => ctx.id_short,
            Self::Author => ctx.author,
            Self::Intent => ctx.intent,
            Self::StateBadges => ctx.state_badges,
            Self::RefBadges => ctx.ref_badges,
            Self::Badges => ctx.badges,
        }
    }
}

impl Default for GraphRenderer {
    fn default() -> Self {
        Self { use_color: true }
    }
}

impl GraphRenderer {
    const ELISION_HEAD_ROWS: usize = 40;
    const ELISION_TAIL_ROWS: usize = 20;

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
        self.render_with_decorations(changes, &GraphDecorations::default())
    }

    /// Render `changes` with optional reference decorations.
    pub fn render_with_decorations(
        &self,
        changes: &[Change],
        decorations: &GraphDecorations,
    ) -> Vec<String> {
        self.render_with_decorations_and_template(changes, decorations, None)
    }

    /// Render `changes` with optional reference decorations and a row template.
    pub fn render_with_decorations_and_template(
        &self,
        changes: &[Change],
        decorations: &GraphDecorations,
        template: Option<&LogTemplate>,
    ) -> Vec<String> {
        let visible: HashSet<ChangeId> = changes.iter().map(|c| ChangeId::from(c.id)).collect();
        let mut active: Vec<ChangeId> = Vec::new();
        let mut rows: Vec<(String, bool)> = Vec::new();

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

            let is_ai = change_is_ai(change);
            let has_conflict = change_has_conflict(change);
            let is_current = decorations.current == Some(id);
            let is_active_head = decorations.active_heads.contains(&id);
            let flags = RowFlags { is_ai, has_conflict, is_current, is_active_head };
            let symbol = self.node_symbol(change, id, decorations);
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

            let line = format!(
                "{} {}",
                graph_prefix,
                self.row_label(change, flags, decorations, template,)
            );

            let has_ref_badges =
                decorations.tags.contains_key(&id) || decorations.remotes.contains_key(&id);
            let linear_simple = parents.len() <= 1
                && !has_ref_badges
                && !is_ai
                && !has_conflict
                && !is_current
                && !is_active_head;

            rows.push((line, linear_simple));

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
                rows.push((self.merge_edge_line(&active, col, parents.len()), false));
            }
        }

        self.collapse_linear_rows(rows)
    }

    fn collapse_linear_rows(&self, rows: Vec<(String, bool)>) -> Vec<String> {
        if rows.len() <= Self::ELISION_HEAD_ROWS + Self::ELISION_TAIL_ROWS {
            return rows.into_iter().map(|(line, _)| line).collect();
        }

        let mut output = Vec::new();
        let mut dropped = 0usize;
        let middle_start = Self::ELISION_HEAD_ROWS;
        let middle_end = rows.len().saturating_sub(Self::ELISION_TAIL_ROWS);

        for (idx, (line, simple)) in rows.into_iter().enumerate() {
            if idx < middle_start || idx >= middle_end {
                if dropped > 0 {
                    output.push(self.elision_row(dropped));
                    dropped = 0;
                }
                output.push(line);
                continue;
            }

            if simple {
                dropped += 1;
            } else {
                if dropped > 0 {
                    output.push(self.elision_row(dropped));
                    dropped = 0;
                }
                output.push(line);
            }
        }

        if dropped > 0 {
            output.push(self.elision_row(dropped));
        }

        output
    }

    fn elision_row(&self, dropped: usize) -> String {
        let row = format!("┆ … {} commits elided …", dropped);
        if self.use_color { row.bright_black().to_string() } else { row }
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

    fn node_symbol(
        &self,
        change: &Change,
        change_id: ChangeId,
        decorations: &GraphDecorations,
    ) -> String {
        let is_ai = change_is_ai(change);
        let has_conflict = change_has_conflict(change);
        let is_current = decorations.current == Some(change_id);
        let is_active_head = decorations.active_heads.contains(&change_id);
        let is_stable = decorations.stable_ancestors.contains(&change_id);

        if !self.use_color {
            return if is_current {
                "@".to_string()
            } else if has_conflict {
                "⊗".to_string()
            } else if is_ai {
                "◉".to_string()
            } else if is_active_head {
                "●".to_string()
            } else {
                "○".to_string()
            };
        }

        if is_current {
            return "@".bright_blue().bold().to_string();
        }
        if is_active_head {
            return "●".bright_green().bold().to_string();
        }

        if has_conflict {
            return "⊗".red().bold().to_string();
        }
        if is_ai {
            return "◉".magenta().bold().to_string();
        }
        if is_stable {
            return "○".bright_black().to_string();
        }
        "○".cyan().to_string()
    }

    fn row_label(
        &self,
        change: &Change,
        flags: RowFlags,
        decorations: &GraphDecorations,
        template: Option<&LogTemplate>,
    ) -> String {
        let change_id = ChangeId::from(change.id);
        let full_id = change_id.to_hex();
        let short_id = change_id.to_hex()[..8].to_string();
        let author = author_label(&change.author);
        let mut state_badges = Vec::new();

        if flags.has_conflict {
            state_badges.push(if self.use_color {
                "⚠".red().bold().to_string()
            } else {
                "⚠".to_string()
            });
        }
        if flags.is_ai {
            state_badges.push(if self.use_color {
                "🤖".magenta().bold().to_string()
            } else {
                "🤖".to_string()
            });
        }
        if flags.is_current {
            state_badges.push(if self.use_color {
                "@".bright_blue().bold().to_string()
            } else {
                "@".to_string()
            });
        }

        let mut ref_badges = Vec::new();
        if let Some(names) = decorations.tags.get(&change_id) {
            for name in names {
                ref_badges.push(self.ref_badge(name, true));
            }
        }
        if let Some(names) = decorations.remotes.get(&change_id) {
            for name in names {
                ref_badges.push(self.ref_badge(name, false));
            }
        }

        let state_badges_text = state_badges.join("");
        let ref_badges_text = ref_badges.join(" ");

        let mut badge_chunks = Vec::new();
        if !state_badges_text.is_empty() {
            badge_chunks.push(state_badges_text.clone());
        }
        if !ref_badges_text.is_empty() {
            badge_chunks.push(ref_badges_text.clone());
        }
        let badges_text = badge_chunks.join(" ");

        if let Some(log_template) = template {
            return log_template.render(TemplateContext {
                id: full_id.as_str(),
                id_short: short_id.as_str(),
                author: author.as_str(),
                intent: change.intent.as_str(),
                state_badges: state_badges_text.as_str(),
                ref_badges: ref_badges_text.as_str(),
                badges: badges_text.as_str(),
            });
        }

        let label = if badges_text.is_empty() {
            format!("{} {} | {}", short_id, author, change.intent)
        } else {
            format!("{} {} {} | {}", short_id, badges_text, author, change.intent)
        };

        if self.use_color
            && decorations.stable_ancestors.contains(&change_id)
            && !flags.is_active_head
            && !flags.is_current
            && !flags.is_ai
            && !flags.has_conflict
        {
            label.bright_black().to_string()
        } else {
            label
        }
    }

    fn ref_badge(&self, name: &str, is_tag: bool) -> String {
        let text = format!("[{}]", name);
        if !self.use_color {
            return text;
        }
        if is_tag { text.cyan().to_string() } else { text.yellow().to_string() }
    }
}

fn dedupe_columns(active: &mut Vec<ChangeId>) {
    let mut seen = HashSet::new();
    active.retain(|id| seen.insert(*id));
}

fn change_has_conflict(change: &Change) -> bool {
    change.atoms.iter().any(|atom| matches!(atom, Atom::Conflict { .. }))
}

fn change_is_ai(change: &Change) -> bool {
    matches!(change.author, Author::AI { .. })
}

fn author_label(author: &Author) -> String {
    match author {
        Author::Human { name, email, .. } => format!("{} <{}>", name, email),
        Author::AI { model, human_sponsor } => {
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

    use arc_algebra_types::Atom;
    use arc_change::Change;
    use arc_store_types::{author::test_keypair, newtypes::ChangeId};

    use super::{GraphDecorations, GraphRenderer, LogTemplate};

    fn mk_change(deps: HashSet<[u8; 32]>, label: &str, with_conflict: bool, ai: bool) -> Change {
        let (author, key) = test_keypair();
        let author = if ai {
            match author {
                arc_store_types::author::Author::Human { key, .. } => {
                    arc_store_types::author::Author::AI {
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
                at: vec!["file".to_string(), format!("{}.rs", label), "fn_x".to_string()],
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
        let has_merge_edge = lines.iter().any(|line| line.contains("├") && line.contains("╮"));
        assert!(has_merge_edge, "merge output must include branch edge connectors");
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

    #[test]
    fn renders_ref_decorations() {
        let root = mk_change(HashSet::new(), "root", false, false);
        let head = mk_change(HashSet::from([root.id]), "head", false, false);

        let mut decorations = GraphDecorations::default();
        decorations.tags.insert(ChangeId::from(head.id), vec!["v1.0.0".to_string()]);
        decorations.remotes.insert(ChangeId::from(head.id), vec!["origin/main".to_string()]);

        let lines =
            GraphRenderer::monochrome().render_with_decorations(&[head, root], &decorations);
        assert!(lines.iter().any(|line| line.contains("[v1.0.0]")));
        assert!(lines.iter().any(|line| line.contains("[origin/main]")));
    }

    #[test]
    fn supports_custom_row_template() {
        let root = mk_change(HashSet::new(), "root", false, false);
        let head = mk_change(HashSet::from([root.id]), "head", false, false);
        let template = LogTemplate::parse("{id_short} {author} => {intent}").unwrap();

        let lines = GraphRenderer::monochrome().render_with_decorations_and_template(
            &[head, root],
            &GraphDecorations::default(),
            Some(&template),
        );

        assert!(lines[0].contains("=> head"));
        assert!(!lines[0].contains(" | "));
    }

    #[test]
    fn rejects_unknown_template_field() {
        let err = LogTemplate::parse("{unknown}").unwrap_err();
        assert!(err.contains("unsupported template field"));
    }

    #[test]
    fn collapses_distant_linear_history_with_elision_row() {
        let mut changes = Vec::new();
        let mut parent = None;

        for i in 0..90 {
            let deps = parent.into_iter().collect();
            let change = mk_change(deps, &format!("c{i}"), false, false);
            parent = Some(change.id);
            changes.push(change);
        }

        let mut newest_first = changes;
        newest_first.reverse();

        let lines = GraphRenderer::monochrome().render(&newest_first);
        assert!(lines.iter().any(|line| line.contains("commits elided")));
    }

    #[test]
    fn template_parse_unclosed_brace() {
        let err = LogTemplate::parse("{id").unwrap_err();
        assert!(err.contains("unclosed"));
    }

    #[test]
    fn template_parse_unmatched_close_brace() {
        let err = LogTemplate::parse("id}").unwrap_err();
        assert!(err.contains("unmatched"));
    }

    #[test]
    fn template_parse_empty_placeholder() {
        let err = LogTemplate::parse("{}").unwrap_err();
        assert!(err.contains("empty placeholder"));
    }

    #[test]
    fn template_parse_all_valid_fields() {
        let tpl = LogTemplate::parse(
            "{id} {id_short} {author} {intent} {state_badges} {ref_badges} {badges}",
        );
        assert!(tpl.is_ok(), "all valid fields must parse");
    }

    #[test]
    fn template_render_literal_only() {
        let tpl = LogTemplate::parse("hello world").unwrap();
        let root = mk_change(HashSet::new(), "root", false, false);
        let lines = GraphRenderer::monochrome().render_with_decorations_and_template(
            &[root],
            &GraphDecorations::default(),
            Some(&tpl),
        );
        assert!(lines[0].contains("hello world"));
    }

    #[test]
    fn single_commit_renders_one_line() {
        let root = mk_change(HashSet::new(), "solo", false, false);
        let lines = GraphRenderer::monochrome().render(&[root]);
        assert_eq!(lines.len(), 1, "single commit must produce exactly one line");
    }

    #[test]
    fn empty_render_produces_no_lines() {
        let lines = GraphRenderer::monochrome().render(&[]);
        assert!(lines.is_empty(), "empty input must produce no output");
    }

    #[test]
    fn graph_renderer_new_is_color() {
        let r = GraphRenderer::new();
        assert!(r.use_color, "default renderer must use color");
    }

    #[test]
    fn graph_renderer_monochrome_is_not_color() {
        let r = GraphRenderer::monochrome();
        assert!(!r.use_color, "monochrome renderer must not use color");
    }

    #[test]
    fn decorations_default() {
        let d = GraphDecorations::default();
        assert!(d.tags.is_empty());
        assert!(d.remotes.is_empty());
        assert!(d.current.is_none());
        assert!(d.active_heads.is_empty());
        assert!(d.stable_ancestors.is_empty());
    }

    #[test]
    fn color_vs_monochrome_single_commit() {
        let root = mk_change(HashSet::new(), "test", false, false);
        let color_lines = GraphRenderer::new().render(std::slice::from_ref(&root));
        let mono_lines = GraphRenderer::monochrome().render(&[root]);
        assert_eq!(color_lines.len(), mono_lines.len());
        // Strip ANSI escape codes before comparing content.
        let strip_ansi = |s: &str| -> String {
            let mut result = String::with_capacity(s.len());
            let mut chars = s.chars().peekable();
            while let Some(c) = chars.next() {
                if c == '\x1b' {
                    while let Some(&next) = chars.peek() {
                        chars.next();
                        if next == 'm' {
                            break;
                        }
                    }
                } else {
                    result.push(c);
                }
            }
            result
        };
        assert_eq!(strip_ansi(&color_lines[0]), mono_lines[0]);
    }

    #[test]
    fn current_checkout_shows_at_symbol() {
        let root = mk_change(HashSet::new(), "root", false, false);
        let decos = GraphDecorations {
            current: Some(ChangeId::from(root.id)),
            ..GraphDecorations::default()
        };
        let lines = GraphRenderer::monochrome().render_with_decorations(&[root], &decos);
        assert!(lines[0].contains("@"), "current commit must show @ symbol");
    }

    #[test]
    fn active_head_shows_dot_symbol() {
        let root = mk_change(HashSet::new(), "root", false, false);
        let mut decos = GraphDecorations::default();
        decos.active_heads.insert(ChangeId::from(root.id));
        let lines = GraphRenderer::monochrome().render_with_decorations(&[root], &decos);
        assert!(lines[0].contains("●"), "active head must show ● symbol");
    }
}
