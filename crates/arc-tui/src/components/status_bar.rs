use ratatui::{
    style::{Modifier, Style},
    widgets::Paragraph,
};

use crate::model::AppState;

pub struct StatusBar;

impl StatusBar {
    pub fn render(frame: &mut ratatui::Frame<'_>, area: ratatui::layout::Rect, state: &AppState) {
        let bar = Paragraph::new(state.status_line.clone())
            .style(Style::default().add_modifier(Modifier::BOLD));
        frame.render_widget(bar, area);
    }
}
