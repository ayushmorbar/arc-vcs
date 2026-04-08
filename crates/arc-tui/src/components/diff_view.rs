use std::collections::HashSet;

use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui_image::picker::ProtocolType;

use crate::components::side_by_side_diff::SideBySideDiff;
use crate::diff::generator::{SemanticDiff, SemanticKind};

pub struct DiffView;

impl DiffView {
    pub fn render(
        frame: &mut ratatui::Frame<'_>,
        area: ratatui::layout::Rect,
        diff: &SemanticDiff,
        side_by_side: &SideBySideDiff,
        selection_mode: bool,
        cursor: usize,
        selected_atoms: &HashSet<usize>,
    ) {
        if let Some(binary) = &diff.binary {
            if let Some(protocol) = detect_image_protocol() {
                let text = format!(
                    "Image preview requested via ratatui-image ({protocol:?})\npath: {}\nhash: {}",
                    binary.path, binary.hash_hex
                );
                let panel = Paragraph::new(text)
                    .block(Block::default().title("Binary Preview").borders(Borders::ALL));
                frame.render_widget(panel, area);
                return;
            }

            let fallback = format!(
                "{}\n{}\npath: {}\nblake3: {}",
                binary.label, "Terminal image protocol unavailable", binary.path, binary.hash_hex
            );
            let panel = Paragraph::new(fallback)
                .block(Block::default().title("Binary Diff").borders(Borders::ALL));
            frame.render_widget(panel, area);
            return;
        }

        if !selection_mode {
            side_by_side.render(frame, area, diff);
            return;
        }

        let split = Layout::horizontal([Constraint::Percentage(62), Constraint::Percentage(38)]).split(area);
        side_by_side.render(frame, split[0], diff);
        Self::render_atom_selector(frame, split[1], diff, cursor, selected_atoms);
    }

    fn render_atom_selector(
        frame: &mut ratatui::Frame<'_>,
        area: ratatui::layout::Rect,
        diff: &SemanticDiff,
        cursor: usize,
        selected_atoms: &HashSet<usize>,
    ) {
        if diff.lines.is_empty() {
            let empty = Paragraph::new("No semantic atoms available")
                .block(Block::default().title("Atom Selection").borders(Borders::ALL));
            frame.render_widget(empty, area);
            return;
        }

        let max_visible = (area.height / 3).max(1) as usize;
        let start = cursor.saturating_sub(max_visible.saturating_sub(1));
        let end = (start + max_visible).min(diff.lines.len());
        let constraints = (start..end)
            .map(|_| Constraint::Length(3))
            .collect::<Vec<_>>();
        let chunks = Layout::vertical(constraints).split(area);

        for (chunk_idx, line_idx) in (start..end).enumerate() {
            let atom = &diff.lines[line_idx];
            let kind = match atom.kind {
                SemanticKind::Insert => "Insert",
                SemanticKind::Delete => "Delete",
                SemanticKind::Modify => "Modify",
                SemanticKind::Unavailable => "Unavailable",
            };

            let mut block = Block::default()
                .title(format!("#{line_idx} {kind} {}", atom.path))
                .borders(Borders::ALL);

            if selected_atoms.contains(&line_idx) {
                block = block.border_style(
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                );
            } else if line_idx == cursor {
                block = block.border_style(Style::default().fg(Color::Yellow));
            }

            let body = match atom.kind {
                SemanticKind::Delete => atom.before.clone(),
                _ => atom.after.clone(),
            };

            let panel = Paragraph::new(body).block(block);
            frame.render_widget(panel, chunks[chunk_idx]);
        }
    }
}

fn detect_image_protocol() -> Option<ProtocolType> {
    if std::env::var("KITTY_WINDOW_ID").is_ok() {
        return Some(ProtocolType::Kitty);
    }
    if std::env::var("ITERM_SESSION_ID").is_ok() {
        return Some(ProtocolType::Iterm2);
    }
    if std::env::var("TERM")
        .map(|v| v.to_ascii_lowercase().contains("sixel"))
        .unwrap_or(false)
    {
        return Some(ProtocolType::Sixel);
    }
    None
}
