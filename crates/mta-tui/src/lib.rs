//! Terminal UI for `mta`.
//!
//! M0 ships the lifecycle, not the board: enter the alternate screen, draw a frame, handle `q`,
//! and restore the terminal on normal exit, error, or panic (PLAN.md §5 M0, §3.8). The render
//! path is a pure `view(&AppState, frame)`; the board, overlays, and hit map land in M2.

#![forbid(unsafe_code)]

pub mod app;
pub mod terminal;

pub use app::App;
pub use terminal::TerminalGuard;
