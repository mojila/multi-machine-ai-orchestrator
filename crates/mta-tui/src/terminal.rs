//! Terminal lifecycle: raw mode, alternate screen, mouse capture, bracketed paste, and an RAII
//! guard that restores all of them on normal exit, error, and panic.
//!
//! The panic hook installed by the CLI calls [`restore_after_panic`] before it prints anything,
//! so a crash leaves a usable terminal and a marker rather than a blank screen
//! (docs/observability.md §2).

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossterm::cursor::Show;
use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::app::App;

static TUI_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Owns the terminal for the lifetime of the TUI.
#[derive(Debug)]
pub struct TerminalGuard {
    active: bool,
}

impl TerminalGuard {
    /// Enters raw mode, the alternate screen, mouse capture, and bracketed paste.
    ///
    /// If any step fails the terminal is restored before the error is returned.
    pub fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        TUI_ACTIVE.store(true, Ordering::SeqCst);
        let mut stdout = io::stdout();
        let entered = execute!(
            stdout,
            EnterAlternateScreen,
            EnableMouseCapture,
            EnableBracketedPaste
        );
        if let Err(error) = entered {
            restore();
            return Err(error);
        }
        Ok(Self { active: true })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if self.active {
            restore();
        }
    }
}

fn restore() {
    if !TUI_ACTIVE.swap(false, Ordering::SeqCst) {
        return;
    }
    let mut stdout = io::stdout();
    // Disable capture before leaving the alternate screen; ignore errors so a broken pipe does
    // not mask the original failure.
    let _ = execute!(
        stdout,
        DisableMouseCapture,
        DisableBracketedPaste,
        LeaveAlternateScreen,
        Show
    );
    let _ = stdout.flush();
    let _ = disable_raw_mode();
}

/// Restores the terminal from a panic hook. Safe to call when the TUI is not active.
pub fn restore_after_panic() {
    restore();
}

/// Runs the event loop until the app quits.
pub fn run(app: &mut App) -> io::Result<()> {
    let _guard = TerminalGuard::enter()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    while !app.should_quit() {
        terminal.draw(|frame| app.view(frame))?;

        // A short poll keeps the loop responsive to resize and quit without a busy spin. This is
        // presentation-only; it never mutates domain state (M2 adds the event-stream select).
        if !event::poll(Duration::from_millis(250))? {
            continue;
        }
        match event::read()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => app.on_key(key),
            Event::Resize(_, _) => {}
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_is_a_noop_when_the_tui_is_not_active() {
        // Must not panic or emit anything outside the TUI.
        restore_after_panic();
        assert!(!TUI_ACTIVE.load(Ordering::SeqCst));
    }
}
