//! `mta` — terminal-first AI agent orchestrator.
//!
//! The binary owns wiring only: argument parsing, config loading, diagnostics setup, and dispatch.
//! The domain, store, provider, engine, and UI live in their own crates and may not depend
//! upward (ADR-0001).

mod cli;
mod commands;
mod config;
mod error;
mod logging;
mod paths;

use std::io::Write;
use std::process::ExitCode;

use clap::Parser;

use crate::cli::{Cli, Command, ConfigAction};
use crate::error::AppError;

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            let code = err.exit_code();
            // Diagnostics are not up yet (or the failure happened while setting them up), so the
            // user-facing error goes to stderr explicitly. `clippy::print_stderr` is denied in
            // library crates; a single top-level error report is the intentional exception and
            // uses an explicit writer rather than `eprintln!`.
            let mut stderr = std::io::stderr().lock();
            let _ = writeln!(stderr, "mta: {err}");
            ExitCode::from(code)
        }
    }
}

fn run(cli: Cli) -> Result<(), AppError> {
    let paths = paths::Paths::discover(cli.config.as_deref())?;
    let loaded = config::LoadedConfig::load(&paths)?;

    let is_tui = matches!(cli.command, None | Some(Command::Tui { .. }));
    let resolved = logging::resolve::resolve(
        &loaded,
        &paths,
        cli.log_level.as_deref(),
        cli.log_format.map(Into::into),
        is_tui,
    )?;
    let logging = logging::init(&resolved)?;

    let flusher = logging.flusher();
    logging::crash::install(
        logging::crash::CrashContext {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            config_hash: loaded.hash.clone(),
            log_path: Some(resolved.dir.clone()),
            crash_dir: resolved.dir.clone(),
            max_field_bytes: resolved.max_field_bytes,
        },
        move || flusher(),
        mta_tui::terminal::restore_after_panic,
    );

    logging::crash::report_and_clear(&resolved.dir, resolved.console);
    tracing::debug!(
        version = env!("CARGO_PKG_VERSION"),
        "diagnostics initialised"
    );

    let result = match cli.command {
        None => commands::tui::run(None),
        Some(Command::Tui { board }) => commands::tui::run(board.as_deref()),
        Some(Command::Doctor(args)) => {
            commands::doctor::run(&paths, &loaded, &resolved, &logging, &args)
        }
        Some(Command::Config { action }) => match action {
            ConfigAction::Init { force } => commands::config_cmd::init(&paths, force),
            ConfigAction::Validate => commands::config_cmd::validate(&loaded, &resolved),
            ConfigAction::Path => commands::config_cmd::path(&paths),
        },
    };

    logging.flush();
    result
}
