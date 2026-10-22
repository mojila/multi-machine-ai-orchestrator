//! The `mta tui` command (also the default command).

use crate::error::AppError;

/// Runs the terminal UI until the user quits.
pub fn run(board: Option<&str>) -> Result<(), AppError> {
    if let Some(board) = board {
        tracing::warn!(
            board,
            "board selection is not implemented until M2; opening the default board"
        );
    }
    let mut app = mta_tui::App::new();
    mta_tui::terminal::run(&mut app).map_err(AppError::Tui)
}
