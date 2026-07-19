use ratatui::widgets::{Block, Borders, Paragraph};

use crate::model::AppState;

pub struct DetailPanel;

impl DetailPanel {
    pub fn render(frame: &mut ratatui::Frame<'_>, area: ratatui::layout::Rect, state: &AppState) {
        let body = if let Some(change) = state.selected_change() {
            format!(
                "Intent\n  {}\n\nAuthor\n  {}\n\nSignature\n  {}\n\nBLAKE3\n  {}",
                change.summary, change.author, change.signature, change.hash
            )
        } else {
            "No change selected".to_string()
        };

        let panel =
            Paragraph::new(body).block(Block::default().title("Detail").borders(Borders::ALL));
        frame.render_widget(panel, area);
    }
}
