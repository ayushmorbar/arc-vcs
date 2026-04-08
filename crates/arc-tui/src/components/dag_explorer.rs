use ratatui::layout::Constraint;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Cell, Row, Table, TableState};

use crate::model::AppState;

#[derive(Default)]
pub struct DagExplorer {
    table_state: TableState,
}

impl DagExplorer {
    pub fn new() -> Self {
        Self {
            table_state: TableState::default(),
        }
    }

    pub fn render(&mut self, frame: &mut ratatui::Frame<'_>, area: ratatui::layout::Rect, state: &AppState) {
        self.table_state.select(Some(state.selected));

        let rows = state.changes.iter().map(|change| {
            Row::new(vec![
                Cell::from(change.id_short.clone()),
                Cell::from(change.summary.clone()),
                Cell::from(change.author.clone()),
            ])
        });

        let table = Table::new(
            rows,
            [
                Constraint::Length(10),
                Constraint::Percentage(55),
                Constraint::Percentage(35),
            ],
        )
        .header(Row::new(vec!["Change", "Intent", "Author"]).style(Style::default().add_modifier(Modifier::BOLD)))
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .column_spacing(1);

        frame.render_stateful_widget(table, area, &mut self.table_state);
    }
}
