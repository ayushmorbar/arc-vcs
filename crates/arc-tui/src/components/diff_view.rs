use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui_image::picker::ProtocolType;

use crate::components::side_by_side_diff::SideBySideDiff;
use crate::diff::generator::SemanticDiff;

pub struct DiffView;

impl DiffView {
    pub fn render(
        frame: &mut ratatui::Frame<'_>,
        area: ratatui::layout::Rect,
        diff: &SemanticDiff,
        side_by_side: &SideBySideDiff,
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

        side_by_side.render(frame, area, diff);
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
