use ratatui::{
    layout::{Constraint, Layout, Rect},
    widgets::{Block, Borders, Paragraph},
};

use crate::diff::generator::SemanticDiff;

#[derive(Debug, Default, Clone)]
pub struct SideBySideDiff {
    scroll: u16,
}

impl SideBySideDiff {
    pub fn scroll_up(&mut self) {
        self.scroll = self.scroll.saturating_sub(1);
    }

    pub fn scroll_down(&mut self) {
        self.scroll = self.scroll.saturating_add(1);
    }

    pub fn render(&self, frame: &mut ratatui::Frame<'_>, area: Rect, diff: &SemanticDiff) {
        let split = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(area);

        let before = Paragraph::new(diff.before.clone())
            .block(Block::default().title("Before").borders(Borders::ALL))
            .scroll((self.scroll, 0));
        let after = Paragraph::new(diff.after.clone())
            .block(Block::default().title("After").borders(Borders::ALL))
            .scroll((self.scroll, 0));

        frame.render_widget(before, split[0]);
        frame.render_widget(after, split[1]);
    }
}
