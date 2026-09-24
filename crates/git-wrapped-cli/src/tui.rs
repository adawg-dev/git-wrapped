mod state;
mod view;

pub use state::{AppState, Page};
pub use view::draw;

use crate::model::RepositoryAnalytics;
use ratatui::{
    backend::CrosstermBackend,
    crossterm::{
        event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
        execute,
        terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    },
    Terminal,
};
use std::io::{self, IsTerminal};

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            LeaveAlternateScreen,
            ratatui::crossterm::cursor::Show
        );
    }
}

pub fn require_terminal() -> Result<(), String> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err("explore requires an interactive terminal".into());
    }
    Ok(())
}

pub fn explore(data: &RepositoryAnalytics) -> Result<(), String> {
    require_terminal()?;
    enable_raw_mode().map_err(|e| e.to_string())?;
    let _restore = TerminalGuard;
    execute!(io::stdout(), EnterAlternateScreen).map_err(|e| e.to_string())?;
    let mut terminal =
        Terminal::new(CrosstermBackend::new(io::stdout())).map_err(|e| e.to_string())?;
    let mut state = AppState::default();
    while state.running {
        terminal
            .draw(|frame| draw(frame, &state, data))
            .map_err(|e| e.to_string())?;
        if let Event::Key(key) = event::read().map_err(|e| e.to_string())? {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                state.running = false;
            } else {
                state.handle_key(key.code, data);
            }
        }
    }
    Ok(())
}
