use std::collections::HashSet;
use std::time::{Duration, Instant};

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
    pub selection_mode: bool,
    pub diff_cursor: usize,
    pub selected_atoms: HashSet<usize>,
    pub sponsorship: Sponsorship,
    pub ghost_intent: Option<String>,
    success_nudge: Option<SuccessNudge>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sponsorship {
    Human,
    Ai,
    Hybrid,
}

impl Sponsorship {
    pub fn cycle(self) -> Self {
        match self {
            Sponsorship::Human => Sponsorship::Ai,
            Sponsorship::Ai => Sponsorship::Hybrid,
            Sponsorship::Hybrid => Sponsorship::Human,
        }
    }

    pub fn as_label(self) -> &'static str {
        match self {
            Sponsorship::Human => "Human",
            Sponsorship::Ai => "AI",
            Sponsorship::Hybrid => "Hybrid",
        }
    }
}

#[derive(Debug)]
struct SuccessNudge {
    hash_short: String,
    started_at: Instant,
    frame: usize,
}

impl AppState {
    pub fn new(changes: Vec<ChangeEntry>) -> Self {
        Self {
            changes,
            selected: 0,
            status_line: "up/down navigate | d diff | q quit".to_string(),
            running: true,
            show_diff: false,
            selection_mode: false,
            diff_cursor: 0,
            selected_atoms: HashSet::new(),
            sponsorship: Sponsorship::Human,
            ghost_intent: None,
            success_nudge: None,
        }
    }

    pub fn select_next(&mut self) {
        if self.changes.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.changes.len();
        self.diff_cursor = 0;
        self.selected_atoms.clear();
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
        self.diff_cursor = 0;
        self.selected_atoms.clear();
    }

    pub fn selected_change(&self) -> Option<&ChangeEntry> {
        self.changes.get(self.selected)
    }

    pub fn selected_diff(&self) -> Option<&SemanticDiff> {
        self.selected_change().and_then(|change| change.diff.as_ref())
    }

    pub fn toggle_selection_mode(&mut self) {
        self.selection_mode = !self.selection_mode;
        if !self.selection_mode {
            self.diff_cursor = 0;
        }
    }

    pub fn move_diff_cursor_down(&mut self) {
        let line_count = self.selected_diff().map(|d| d.lines.len()).unwrap_or(0);
        if line_count == 0 {
            self.diff_cursor = 0;
            return;
        }
        self.diff_cursor = (self.diff_cursor + 1) % line_count;
    }

    pub fn move_diff_cursor_up(&mut self) {
        let line_count = self.selected_diff().map(|d| d.lines.len()).unwrap_or(0);
        if line_count == 0 {
            self.diff_cursor = 0;
            return;
        }
        self.diff_cursor = if self.diff_cursor == 0 {
            line_count - 1
        } else {
            self.diff_cursor - 1
        };
    }

    pub fn toggle_selected_atom(&mut self) {
        let line_count = self.selected_diff().map(|d| d.lines.len()).unwrap_or(0);
        if line_count == 0 {
            return;
        }
        if self.selected_atoms.contains(&self.diff_cursor) {
            self.selected_atoms.remove(&self.diff_cursor);
        } else {
            self.selected_atoms.insert(self.diff_cursor);
        }
    }

    pub fn cycle_sponsorship(&mut self) {
        self.sponsorship = self.sponsorship.cycle();
    }

    pub fn ghost_text(&self) -> String {
        if let Some(intent) = &self.ghost_intent {
            return intent.clone();
        }

        let Some(diff) = self.selected_diff() else {
            return "Intent: Describe the semantic change you want to capture".to_string();
        };

        let mut insert_count = 0usize;
        let mut delete_count = 0usize;
        let mut modify_count = 0usize;
        for line in &diff.lines {
            match line.kind {
                crate::diff::generator::SemanticKind::Insert => insert_count += 1,
                crate::diff::generator::SemanticKind::Delete => delete_count += 1,
                crate::diff::generator::SemanticKind::Modify => modify_count += 1,
                crate::diff::generator::SemanticKind::Unavailable => {}
            }
        }

        format!(
            "Refactor: {} insert, {} delete, {} modify across semantic atoms",
            insert_count, delete_count, modify_count
        )
    }

    pub fn inject_intent_event(&mut self, intent: String) {
        self.ghost_intent = Some(intent);
    }

    pub fn selected_atom_count(&self) -> usize {
        self.selected_atoms.len()
    }

    pub fn start_success_nudge(&mut self, hash_short: String) {
        self.success_nudge = Some(SuccessNudge {
            hash_short,
            started_at: Instant::now(),
            frame: 0,
        });
    }

    pub fn tick_animations(&mut self) {
        let Some(nudge) = &mut self.success_nudge else {
            return;
        };

        let elapsed = nudge.started_at.elapsed();
        if elapsed > Duration::from_millis(850) {
            self.status_line = format!("✔ Spacetime captured: {}", nudge.hash_short);
            self.success_nudge = None;
            return;
        }

        let frames = ['-', '\\', '|', '/'];
        let glyph = frames[nudge.frame % frames.len()];
        nudge.frame += 1;
        self.status_line = format!("{glyph} Spacetime captured: {}", nudge.hash_short);
    }

    pub fn apply_output_event(&mut self, event: OutputEvent) {
        match event {
            OutputEvent::Started(op) => {
                self.status_line = format!("running {op}");
            }
            OutputEvent::Progress(current, total, message) => {
                if let Some(intent) = message.strip_prefix("intent:") {
                    self.ghost_intent = Some(intent.trim().to_string());
                }
                self.status_line = format!("{message} ({current}/{total})");
            }
            OutputEvent::Success(summary, _) => {
                self.status_line = format!("ok {summary}");
            }
            OutputEvent::Warning(message) => {
                if let Some(intent) = message.strip_prefix("intent:") {
                    self.ghost_intent = Some(intent.trim().to_string());
                }
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
    ToggleSelectionMode,
    ToggleAtom,
    SponsorshipNext,
    SnapNow,
    IntentEvent(String),
    Backend(OutputEvent),
}

impl PartialEq for Message {
    fn eq(&self, other: &Self) -> bool {
        use Message::{Backend, IntentEvent, MoveDown, MoveUp, OpenDiff, Quit, SnapNow, SponsorshipNext, Tick, ToggleAtom, ToggleSelectionMode};
        match (self, other) {
            (Tick, Tick)
            | (Quit, Quit)
            | (MoveUp, MoveUp)
            | (MoveDown, MoveDown)
            | (OpenDiff, OpenDiff)
            | (ToggleSelectionMode, ToggleSelectionMode)
            | (ToggleAtom, ToggleAtom)
            | (SponsorshipNext, SponsorshipNext)
            | (SnapNow, SnapNow) => true,
            (IntentEvent(a), IntentEvent(b)) => a == b,
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

    #[test]
    fn intent_event_is_mapped_to_ghost_text() {
        let mut state = AppState::new(sample_changes());
        state.apply_output_event(OutputEvent::Warning(
            "intent: Refactor: Optimized blake3 hashing in core".to_string(),
        ));
        assert_eq!(
            state.ghost_text(),
            "Refactor: Optimized blake3 hashing in core"
        );
    }
}
