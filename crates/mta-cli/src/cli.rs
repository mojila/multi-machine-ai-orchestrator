//! Command-line surface. M0 defines the global flags, `doctor`, `config`, and `tui`; the rest of
//! the surface documented in PLAN.md §2.4 lands with the milestone that implements it.

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

use crate::config::LogFormat;

/// Terminal-first AI agent orchestrator: a Kanban board whose cards are executed by agents.
#[derive(Debug, Parser)]
#[command(
    name = "mta",
    version,
    about,
    long_about = None,
    disable_help_subcommand = true
)]
pub struct Cli {
    /// Log level: error | warn | info | debug | trace | off.
    ///
    /// Overrides RUST_LOG and `[logging].level`.
    #[arg(long, global = true, value_name = "LEVEL")]
    pub log_level: Option<String>,

    /// Log format: human | json.
    #[arg(long, global = true, value_name = "FORMAT")]
    pub log_format: Option<LogFormatArg>,

    /// Config file path (default: $XDG_CONFIG_HOME/mta/config.toml).
    #[arg(long, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Database path (default: $XDG_DATA_HOME/mta/mta.db). Recorded in M0; used from M1.
    #[arg(long, global = true, value_name = "PATH")]
    pub db: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Launch the Kanban TUI (the default command).
    Tui {
        /// Board to open. Only `work` exists until M2.
        #[arg(long, value_name = "NAME")]
        board: Option<String>,
    },

    /// Check config, paths, and diagnostics.
    Doctor(DoctorArgs),

    /// Inspect and initialise configuration.
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}

#[derive(Debug, clap::Args)]
pub struct DoctorArgs {
    /// Report resolved logging settings, sinks, log files, and recent records.
    #[arg(long)]
    pub logs: bool,
}

#[derive(Debug, Subcommand)]
pub enum ConfigAction {
    /// Write a commented default config. Refuses to overwrite unless --force.
    Init {
        /// Overwrite an existing config file.
        #[arg(long)]
        force: bool,
    },
    /// Parse and validate the config (including logging directives).
    Validate,
    /// Print the config file path.
    Path,
}

/// CLI spelling of the log format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum LogFormatArg {
    Human,
    Json,
}

impl From<LogFormatArg> for LogFormat {
    fn from(value: LogFormatArg) -> Self {
        match value {
            LogFormatArg::Human => Self::Human,
            LogFormatArg::Json => Self::Json,
        }
    }
}
