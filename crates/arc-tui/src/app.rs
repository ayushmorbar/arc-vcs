use std::io;
use std::time::Duration;

use anyhow::Context;
use crossterm::event::{self, Event as CEvent, KeyCode};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{execute, ExecutableCommand};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::mpsc;
use tuirealm::{Application, EventListenerCfg, NoUserEvent, PollStrategy};
use arc_ux::OutputEvent;

use crate::components::dag_explorer::DagExplorer;
use crate::components::detail_panel::DetailPanel;
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
    backend_events: mpsc::Receiver<OutputEvent>,
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
        Self {
            state: AppState::new(provider.list_changes()),
            dag: DagExplorer::new(),
            backend_events,
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
            self.realm_tick();

            terminal
                .draw(|frame| {
                    let bento = split_bento(frame.area());
                    self.dag.render(frame, bento.dag, &self.state);
                    DetailPanel::render(frame, bento.detail, &self.state);
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

    fn handle_input(&mut self, input: CEvent) {
        if let CEvent::Key(key) = input {
            match key.code {
                KeyCode::Char('q') => self.handle_message(Message::Quit),
                KeyCode::Down => self.handle_message(Message::MoveDown),
                KeyCode::Up => self.handle_message(Message::MoveUp),
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
                self.state.status_line = "diff action placeholder".to_string();
            }
            Message::Backend(event) => self.state.apply_output_event(event),
        }
    }
}
