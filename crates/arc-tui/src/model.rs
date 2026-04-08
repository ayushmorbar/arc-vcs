use arc_ux::OutputEvent;
use arc_change::Change;

use crate::diff::generator::SemanticDiff;

#[derive(Debug, Clone)]
pub struct ChangeEntry {
    pub id_short: String,
    pub summary: String,
    pub author: String,
    pub signature: String,
    pub hash: String,
    pub change: Change,
    pub diff: Option<SemanticDiff>,
}

#[derive(Debug)]
pub struct AppState {
    pub changes: Vec<ChangeEntry>,
    pub selected: usize,
    pub status_line: String,
    pub running: bool,
    pub show_diff: bool,
}

impl AppState {
    pub fn new(changes: Vec<ChangeEntry>) -> Self {
        Self {
            changes,
            selected: 0,
            status_line: "up/down navigate | d diff | q quit".to_string(),
            running: true,
            show_diff: false,
        }
    }

    pub fn select_next(&mut self) {
        if self.changes.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.changes.len();
    }

    pub fn select_prev(&mut self) {
        if self.changes.is_empty() {
            return;
        }
        self.selected = if self.selected == 0 {
            self.changes.len() - 1
        } else {
            self.selected - 1
        };
    }

    pub fn selected_change(&self) -> Option<&ChangeEntry> {
        self.changes.get(self.selected)
    }

    pub fn selected_diff(&self) -> Option<&SemanticDiff> {
        self.selected_change().and_then(|change| change.diff.as_ref())
    }

    pub fn apply_output_event(&mut self, event: OutputEvent) {
        match event {
            OutputEvent::Started(op) => {
                self.status_line = format!("running {op}");
            }
            OutputEvent::Progress(current, total, message) => {
                self.status_line = format!("{message} ({current}/{total})");
            }
            OutputEvent::Success(summary, _) => {
                self.status_line = format!("ok {summary}");
            }
            OutputEvent::Warning(message) => {
                self.status_line = format!("warn {message}");
            }
            OutputEvent::Diagnostic(report) => {
                self.status_line = format!("err {}", report);
            }
        }
    }
}

pub enum Message {
    Tick,
    Quit,
    MoveUp,
    MoveDown,
    OpenDiff,
    Backend(OutputEvent),
}

impl PartialEq for Message {
    fn eq(&self, other: &Self) -> bool {
        use Message::{Backend, MoveDown, MoveUp, OpenDiff, Quit, Tick};
        match (self, other) {
            (Tick, Tick) | (Quit, Quit) | (MoveUp, MoveUp) | (MoveDown, MoveDown) | (OpenDiff, OpenDiff) => true,
            (Backend(a), Backend(b)) => output_event_eq(a, b),
            _ => false,
        }
    }
}

fn output_event_eq(a: &OutputEvent, b: &OutputEvent) -> bool {
    match (a, b) {
        (OutputEvent::Started(x), OutputEvent::Started(y)) => x == y,
        (OutputEvent::Progress(ac, at, am), OutputEvent::Progress(bc, bt, bm)) => {
            ac == bc && at == bt && am == bm
        }
        (OutputEvent::Success(asum, adet), OutputEvent::Success(bsum, bdet)) => {
            asum == bsum && adet == bdet
        }
        (OutputEvent::Warning(x), OutputEvent::Warning(y)) => x == y,
        (OutputEvent::Diagnostic(x), OutputEvent::Diagnostic(y)) => x.to_string() == y.to_string(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use arc_change::Change;
    use arc_store_types::author;

    use super::{AppState, ChangeEntry};
    use arc_ux::OutputEvent;

    fn sample_change() -> Change {
        let (author, signing_key) = author::test_keypair();
        Change::new(HashSet::new(), vec![], "sample", author, &signing_key)
    }

    fn sample_changes() -> Vec<ChangeEntry> {
        vec![
            ChangeEntry {
                id_short: "abc0001".to_string(),
                summary: "first".to_string(),
                author: "alice".to_string(),
                signature: "ok".to_string(),
                hash: "h1".to_string(),
                change: sample_change(),
                diff: None,
            },
            ChangeEntry {
                id_short: "abc0002".to_string(),
                summary: "second".to_string(),
                author: "bob".to_string(),
                signature: "ok".to_string(),
                hash: "h2".to_string(),
                change: sample_change(),
                diff: None,
            },
        ]
    }

    #[test]
    fn selection_wraps_forward_and_backward() {
        let mut state = AppState::new(sample_changes());

        assert_eq!(state.selected, 0);
        state.select_prev();
        assert_eq!(state.selected, 1);
        state.select_next();
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn backend_started_event_updates_status_line() {
        let mut state = AppState::new(sample_changes());
        state.apply_output_event(OutputEvent::Started("sync".to_string()));
        assert_eq!(state.status_line, "running sync");
    }

    #[test]
    fn selected_change_returns_current_item() {
        let state = AppState::new(sample_changes());
        let selected = state.selected_change().expect("selected change");
        assert_eq!(selected.id_short, "abc0001");
    }
}
