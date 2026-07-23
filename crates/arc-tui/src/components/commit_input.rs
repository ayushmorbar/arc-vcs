use crossterm::event::KeyEvent;
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};
use ratatui_textarea::TextArea;

pub struct CommitInput {
    textarea: TextArea<'static>,
}

impl Default for CommitInput {
    fn default() -> Self {
        Self::new()
    }
}

impl CommitInput {
    pub fn new() -> Self {
        let mut textarea = TextArea::default();
        textarea.set_block(Block::default().borders(Borders::ALL).title("Intent"));
        Self { textarea }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        self.textarea.input(key)
    }

    pub fn intent_text(&self) -> String {
        self.textarea.lines().join("\n")
    }

    pub fn is_empty(&self) -> bool {
        self.intent_text().trim().is_empty()
    }

    pub fn render(
        &mut self,
        frame: &mut ratatui::Frame<'_>,
        area: ratatui::layout::Rect,
        ghost_text: &str,
    ) {
        frame.render_widget(&self.textarea, area);

        if self.is_empty() {
            let ghost = Paragraph::new(Line::from(vec![Span::styled(
                ghost_text.to_string(),
                Style::default().add_modifier(Modifier::DIM),
            )]))
            .block(Block::default().borders(Borders::ALL).title("Intent"));
            frame.render_widget(ghost, area);
        }
    }
}
