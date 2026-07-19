use ratatui::layout::{Constraint, Layout, Rect};

#[derive(Debug, Clone, Copy)]
pub struct BentoLayout {
    pub dag: Rect,
    pub detail: Rect,
    pub status: Rect,
}

pub fn split_bento(area: Rect) -> BentoLayout {
    let vertical = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(area);
    let top = Layout::horizontal([Constraint::Percentage(70), Constraint::Percentage(30)])
        .split(vertical[0]);

    BentoLayout { dag: top[0], detail: top[1], status: vertical[1] }
}
