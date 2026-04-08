use std::io;
use std::sync::mpsc as std_mpsc;
use std::time::Duration;

use anyhow::Context;
use arc_keyring::{IdentityManager, KeyringSessionFacade};
use crossterm::event::{self, Event as CEvent, KeyCode, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{execute, ExecutableCommand};
use ratatui::backend::CrosstermBackend;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Terminal;
use tokio::sync::mpsc;
use tuirealm::{Application, EventListenerCfg, NoUserEvent, PollStrategy};
use arc_ux::OutputEvent;

use crate::components::commit_input::CommitInput;
use crate::components::dag_explorer::DagExplorer;
use crate::components::detail_panel::DetailPanel;
use crate::components::diff_view::DiffView;
use crate::components::side_by_side_diff::SideBySideDiff;
use crate::components::status_bar::StatusBar;
use crate::layout::split_bento;
use crate::model::{AppState, Message};
use crate::provider::ChangeProvider;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum RealmView {
    Root,
}

pub struct App {
    state: AppState,
    dag: DagExplorer,
    diff: SideBySideDiff,
    commit_input: CommitInput,
    backend_events: mpsc::Receiver<OutputEvent>,
    ghost_intent_tx: std_mpsc::Sender<(u64, String)>,
    ghost_intent_rx: std_mpsc::Receiver<(u64, String)>,
    ghost_request_token: u64,
    realm: Application<RealmView, Message, NoUserEvent>,
}

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let mut stdout = io::stdout();
        let _ = stdout.execute(LeaveAlternateScreen);
    }
}

impl App {
    pub fn new(provider: impl ChangeProvider, backend_events: mpsc::Receiver<OutputEvent>) -> Self {
        let (ghost_intent_tx, ghost_intent_rx) = std_mpsc::channel();
        Self {
            state: AppState::new(provider.list_changes()),
            dag: DagExplorer::new(),
            diff: SideBySideDiff::default(),
            commit_input: CommitInput::new(),
            backend_events,
            ghost_intent_tx,
            ghost_intent_rx,
            ghost_request_token: 0,
            realm: Application::init(EventListenerCfg::default()),
        }
    }

    pub fn run(&mut self) -> anyhow::Result<()> {
        let mut stdout = io::stdout();
        enable_raw_mode().context("failed to enable raw mode")?;
        execute!(stdout, EnterAlternateScreen).context("failed to enter alternate screen")?;
        let _guard = TerminalGuard;

        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend).context("failed to create terminal")?;
        let frame_budget = Duration::from_millis(16);

        while self.state.running {
            self.drain_backend_events();
            self.drain_ghost_intent_events();
            self.realm_tick();
            self.state.tick_animations();

            terminal
                .draw(|frame| {
                    let bento = split_bento(frame.area());

                    if self.state.show_diff {
                        let workspace = ratatui::layout::Rect {
                            x: bento.dag.x,
                            y: bento.dag.y,
                            width: bento.dag.width + bento.detail.width,
                            height: bento.dag.height,
                        };

                        let split = ratatui::layout::Layout::vertical([
                            ratatui::layout::Constraint::Length(2),
                            ratatui::layout::Constraint::Min(8),
                            ratatui::layout::Constraint::Length(4),
                        ])
                        .split(workspace);

                        let header = Paragraph::new(format!(
                            "Sponsorship: {} | Selection: {} | Selected Atoms: {}",
                            self.state.sponsorship.as_label(),
                            if self.state.selection_mode { "on" } else { "off" },
                            self.state.selected_atom_count()
                        ))
                        .block(Block::default().title("Snap Header").borders(Borders::ALL));
                        frame.render_widget(header, split[0]);

                        if let Some(diff) = self.state.selected_diff() {
                            DiffView::render(
                                frame,
                                split[1],
                                diff,
                                &self.diff,
                                self.state.selection_mode,
                                self.state.diff_cursor,
                                &self.state.selected_atoms,
                            );
                        } else {
                            DetailPanel::render(frame, split[1], &self.state);
                        }

                        let ghost = self.state.ghost_text();
                        self.commit_input.render(frame, split[2], &ghost);
                    } else {
                        self.dag.render(frame, bento.dag, &self.state);
                        DetailPanel::render(frame, bento.detail, &self.state);
                    }

                    StatusBar::render(frame, bento.status, &self.state);
                })
                .context("failed to draw frame")?;

            if event::poll(frame_budget).context("input polling failed")? {
                let input = event::read().context("input read failed")?;
                self.handle_input(input);
            }
        }
        Ok(())
    }

    fn realm_tick(&mut self) {
        let _root = RealmView::Root;
        let _ = self.realm.tick(PollStrategy::Once);
    }

    fn drain_backend_events(&mut self) {
        while let Ok(event) = self.backend_events.try_recv() {
            self.handle_message(Message::Backend(event));
        }
    }

    fn drain_ghost_intent_events(&mut self) {
        while let Ok((token, intent)) = self.ghost_intent_rx.try_recv() {
            if token != self.ghost_request_token {
                continue;
            }
            self.handle_message(Message::IntentEvent(intent));
        }
    }

    fn handle_input(&mut self, input: CEvent) {
        if let CEvent::Key(key) = input {
            if self.state.show_diff {
                match key.code {
                    KeyCode::Esc => {
                        self.handle_message(Message::OpenDiff);
                        return;
                    }
                    KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.handle_message(Message::ToggleSelectionMode);
                        return;
                    }
                    KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.handle_message(Message::SponsorshipNext);
                        return;
                    }
                    KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.handle_message(Message::ToggleAtom);
                        return;
                    }
                    KeyCode::Enter => {
                        self.handle_message(Message::SnapNow);
                        return;
                    }
                    KeyCode::Up => {
                        if self.state.selection_mode {
                            self.state.move_diff_cursor_up();
                        } else {
                            self.diff.scroll_up();
                        }
                        return;
                    }
                    KeyCode::Down => {
                        if self.state.selection_mode {
                            self.state.move_diff_cursor_down();
                        } else {
                            self.diff.scroll_down();
                        }
                        return;
                    }
                    _ => {}
                }

                let _ = self.commit_input.handle_key(key);
                return;
            }

            match key.code {
                KeyCode::Char('q') => self.handle_message(Message::Quit),
                KeyCode::Down => {
                    self.handle_message(Message::MoveDown);
                }
                KeyCode::Up => {
                    self.handle_message(Message::MoveUp);
                }
                KeyCode::Char('d') => self.handle_message(Message::OpenDiff),
                _ => {}
            }
        }
    }

    fn handle_message(&mut self, msg: Message) {
        match msg {
            Message::Tick => {}
            Message::Quit => self.state.running = false,
            Message::MoveUp => self.state.select_prev(),
            Message::MoveDown => self.state.select_next(),
            Message::OpenDiff => {
                self.state.show_diff = !self.state.show_diff;
                self.state.status_line = if self.state.show_diff {
                    "Snap mode | Enter capture | Ctrl+V atom-select | Ctrl+T toggle atom | Ctrl+S sponsor | Esc back"
                        .to_string()
                } else {
                    "up/down navigate | d diff | q quit".to_string()
                };
                if self.state.show_diff {
                    self.spawn_ghost_intent_fetch();
                }
            }
            Message::ToggleSelectionMode => {
                self.state.toggle_selection_mode();
                self.state.status_line = if self.state.selection_mode {
                    "Selection mode on | up/down move | space toggle atom".to_string()
                } else {
                    "Selection mode off".to_string()
                };
            }
            Message::ToggleAtom => {
                self.state.toggle_selected_atom();
                self.state.status_line = format!(
                    "Selected atoms: {}",
                    self.state.selected_atom_count()
                );
            }
            Message::SponsorshipNext => {
                self.state.cycle_sponsorship();
                self.state.status_line = format!(
                    "Sponsorship set to {}",
                    self.state.sponsorship.as_label()
                );
            }
            Message::SnapNow => {
                let intent_text = self.commit_input.intent_text();
                let intent = if intent_text.trim().is_empty() {
                    self.state.ghost_text()
                } else {
                    intent_text
                };
                let metadata = build_intent_metadata(
                    &intent,
                    self.state.sponsorship.as_label(),
                    &self.state.selected_atoms,
                );

                match sign_intent_metadata(metadata.as_bytes()) {
                    Ok(signature) => {
                        let short = self
                            .state
                            .selected_change()
                            .map(|change| change.hash.chars().take(6).collect::<String>())
                            .unwrap_or_else(|| "a1b2c3".to_string());
                        self.state.start_success_nudge(short.clone());
                        self.state.status_line = format!(
                            "intent signed {}... | finalizing spacetime capture",
                            &signature[..8]
                        );
                    }
                    Err(error) => {
                        self.state.status_line = format!("snap signing failed: {error}");
                    }
                }
            }
            Message::IntentEvent(intent) => {
                self.state.inject_intent_event(intent.clone());
                self.state.status_line = format!("Ghostwriter summary: {intent}");
            }
            Message::Backend(event) => self.state.apply_output_event(event),
        }
    }

    fn spawn_ghost_intent_fetch(&mut self) {
        let Some(diff) = self.state.selected_diff() else {
            return;
        };

        let summary = summarize_diff_for_ghostwriter(diff);
        self.ghost_request_token = self.ghost_request_token.saturating_add(1);
        let token = self.ghost_request_token;
        let tx = self.ghost_intent_tx.clone();
        self.state.status_line = "Ghostwriter is analyzing semantic diff...".to_string();

        std::thread::spawn(move || {
            let intent = match tokio::runtime::Runtime::new() {
                Ok(runtime) => runtime
                    .block_on(arc_ai::generate_ghost_intent(&summary))
                    .unwrap_or_else(|_| fallback_ghost_intent(&summary)),
                Err(_) => fallback_ghost_intent(&summary),
            };
            let _ = tx.send((token, intent));
        });
    }
}

fn summarize_diff_for_ghostwriter(diff: &crate::diff::generator::SemanticDiff) -> String {
    if diff.lines.is_empty() {
        return "No semantic atoms changed".to_string();
    }

    let mut lines = Vec::with_capacity(diff.lines.len());
    for line in &diff.lines {
        let kind = match line.kind {
            crate::diff::generator::SemanticKind::Insert => "Insert",
            crate::diff::generator::SemanticKind::Delete => "Delete",
            crate::diff::generator::SemanticKind::Modify => "Modify",
            crate::diff::generator::SemanticKind::Unavailable => "Unavailable",
        };
        lines.push(format!("{kind}: {}", line.path));
    }
    lines.join("\n")
}

fn fallback_ghost_intent(diff_summary: &str) -> String {
    let lower = diff_summary.to_ascii_lowercase();
    if lower.contains("network") && lower.contains("sync") {
        return "Refactor: Modularized network sync logic".to_string();
    }
    "Refactor: Summarized semantic changes for snap intent".to_string()
}

fn sign_intent_metadata(payload: &[u8]) -> Result<String, String> {
    let manager = IdentityManager::init()
        .map_err(|error| format!("keyring init failed: {error}"))?;
    let facade = KeyringSessionFacade::new(manager);
    let alias = facade
        .active_alias()
        .map_err(|error| format!("identity session read failed: {error}"))?
        .ok_or_else(|| {
            "No active identity loaded. Select/Generate Identity via 'arc auth login' before snapping.".to_string()
        })?;

    let passphrase = std::env::var("ARC_KEYRING_PASSPHRASE").map_err(|_| {
        format!(
            "Identity '{alias}' is selected but locked. Set ARC_KEYRING_PASSPHRASE to sign snaps."
        )
    })?;

    facade
        .manager()
        .load(&alias, &passphrase)
        .map_err(|error| format!("identity load failed: {error}"))?;

    let signature = facade
        .manager()
        .sign(payload)
        .map_err(|error| format!("signing failed: {error}"))?;
    Ok(hex_encode(&signature.to_bytes()))
}

fn build_intent_metadata(intent: &str, sponsorship: &str, selected_atoms: &std::collections::HashSet<usize>) -> String {
    let mut selected = selected_atoms.iter().copied().collect::<Vec<_>>();
    selected.sort_unstable();
    format!(
        "intent={intent}\nsponsorship={sponsorship}\nselected_atoms={selected:?}\n"
    )
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::build_intent_metadata;

    #[test]
    fn metadata_serializes_selected_atoms_in_stable_order() {
        let mut selected = HashSet::new();
        selected.insert(3);
        selected.insert(1);
        selected.insert(2);

        let metadata = build_intent_metadata("refactor", "Hybrid", &selected);
        assert!(metadata.contains("selected_atoms=[1, 2, 3]"));
    }
}
