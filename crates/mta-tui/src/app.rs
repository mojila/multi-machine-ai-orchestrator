//! Application state and the pure render function.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mta_domain::Secret;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

/// M0 application state.
///
/// The board view model replaces this in M2; keeping the quit flag and the view in one small
/// type makes the lifecycle testable without a terminal.
#[derive(Debug, Default)]
pub struct App {
    should_quit: bool,
}

impl App {
    /// Creates the application.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `true` when the event loop should stop.
    #[must_use]
    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    /// Applies a key event. `q` and `Ctrl-C` quit, as documented in the keyboard model.
    pub fn on_key(&mut self, key: KeyEvent) {
        let quit = matches!(key.code, KeyCode::Char('q'))
            || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL));
        if quit {
            self.should_quit = true;
        }
    }

    /// Renders a frame. Pure: no I/O, no state mutation.
    pub fn view(&self, frame: &mut Frame<'_>) {
        let [header, body, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .areas(frame.area());

        let title = Line::from(vec![
            Span::styled("mta", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" · bootstrap HUD · M0"),
        ]);
        frame.render_widget(Paragraph::new(title).alignment(Alignment::Left), header);

        let help = vec![
            Line::from("The Kanban board lands in M2."),
            Line::from(""),
            Line::from("q  quit"),
        ];
        frame.render_widget(
            Paragraph::new(help)
                .block(Block::default().borders(Borders::ALL).title(" mta "))
                .wrap(Wrap { trim: true }),
            body,
        );

        let version = format!(
            "mta {} · {}",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS
        );
        frame.render_widget(Paragraph::new(version).alignment(Alignment::Right), footer);
    }
}

/// A secret stored in the app state must never render, even if it is logged by mistake.
/// This function exists so the type is exercised by the TUI crate's tests; it is not drawn.
#[must_use]
pub fn redacted_for_display(secret: &Secret) -> String {
    format!("{secret}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn q_quits_and_other_keys_do_not() {
        let mut app = App::new();
        app.on_key(KeyEvent::from(KeyCode::Char('j')));
        assert!(!app.should_quit());
        app.on_key(KeyEvent::from(KeyCode::Char('q')));
        assert!(app.should_quit());
    }

    #[test]
    fn ctrl_c_quits() {
        let mut app = App::new();
        app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app.should_quit());
    }

    #[test]
    fn secret_never_renders() {
        let secret = Secret::new("sk-tui-must-not-show-this");
        assert_eq!(redacted_for_display(&secret), "<redacted>");
    }
}
