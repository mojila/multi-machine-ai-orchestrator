//! Resolve configured, environment, and CLI diagnostics settings into one effective plan.
//!
//! Precedence (docs/observability.md §3): `--log-level` > `RUST_LOG` > `[logging]` > `info`.
//! When `RUST_LOG` is present it replaces both the global level and the directive list; when it
//! has no bare level, the configured global level remains in force. `doctor --logs` prints the
//! origin of each value so the rule is inspectable rather than folklore.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use crate::config::{AnsiMode, LoadedConfig, LogFormat, Rotation};
use crate::error::AppError;
use crate::logging::filter::{self, FilterSpec};
use crate::paths::Paths;

/// Where an effective value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Compiled-in default (or a config file that was not present).
    Default,
    /// The config file.
    Config,
    /// The `RUST_LOG` environment variable.
    RustLog,
    /// A command-line flag.
    Cli,
}

impl Source {
    /// A human description suitable for `doctor --logs`.
    #[must_use]
    pub fn describe(self, config_path: &Path) -> String {
        match self {
            Self::Default => "default".to_owned(),
            Self::Config => format!("from {}", config_path.display()),
            Self::RustLog => "from RUST_LOG".to_owned(),
            Self::Cli => "from --log-level".to_owned(),
        }
    }
}

/// The effective diagnostics plan.
#[derive(Debug, Clone)]
pub struct ResolvedLogging {
    /// The combined filter.
    pub filter: FilterSpec,
    /// Origin of the global level.
    pub level_source: Source,
    /// The directive list in effect (already parsed).
    pub directives: Vec<String>,
    /// Origin of the directive list.
    pub directives_source: Source,
    /// Effective record format.
    pub format: LogFormat,
    /// Origin of the format.
    pub format_source: Source,
    /// Configured ANSI policy.
    pub ansi_mode: AnsiMode,
    /// Whether ANSI escapes will be emitted.
    pub ansi: bool,
    /// Whether the stderr sink is on.
    pub console: bool,
    /// True when TUI mode disabled a configured console sink.
    pub console_forced_off: bool,
    /// Whether the file sink is on.
    pub file: bool,
    /// True when TUI mode forced the file sink on.
    pub file_forced_on: bool,
    /// Effective log directory.
    pub dir: PathBuf,
    /// Rotation cadence.
    pub rotation: Rotation,
    /// Files kept per rotation series.
    pub retain_files: usize,
    /// Per-file size cap.
    pub max_file_mb: u64,
    /// Directory size cap.
    pub max_total_mb: u64,
    /// TUI overlay ring size (consumed in M2).
    pub ring_lines: usize,
    /// Prompt text opt-in.
    pub log_prompts: bool,
    /// Tool output opt-in.
    pub log_tool_output: bool,
    /// Field truncation cap.
    pub max_field_bytes: usize,
    /// Dedup window (consumed in M2).
    pub dedup_window_ms: u64,
    /// The config file consulted.
    pub config_path: PathBuf,
}

/// Resolves settings using `RUST_LOG` from the process environment.
pub fn resolve(
    loaded: &LoadedConfig,
    paths: &Paths,
    cli_level: Option<&str>,
    cli_format: Option<LogFormat>,
    tui: bool,
) -> Result<ResolvedLogging, AppError> {
    let rust_log = std::env::var("RUST_LOG").ok();
    resolve_with_env(
        loaded,
        paths,
        cli_level,
        cli_format,
        tui,
        rust_log.as_deref(),
    )
}

/// Resolves settings with an explicit `RUST_LOG` value (testable without mutating the process).
pub fn resolve_with_env(
    loaded: &LoadedConfig,
    paths: &Paths,
    cli_level: Option<&str>,
    cli_format: Option<LogFormat>,
    tui: bool,
    rust_log: Option<&str>,
) -> Result<ResolvedLogging, AppError> {
    let config = &loaded.config.logging;
    let config_source = if loaded.exists {
        Source::Config
    } else {
        Source::Default
    };

    let config_level = filter::parse_level(&config.level)
        .map_err(|error| AppError::Config(format!("logging.level: {error}")))?;
    let config_directives = filter::parse_directives(&config.directives)
        .map_err(|error| AppError::Config(format!("logging.directives: {error}")))?;

    // An empty RUST_LOG is conventionally "unset"; treating it as a filter would silence logs.
    let rust_log = rust_log.filter(|value| !value.trim().is_empty());
    let env_parsed = rust_log
        .map(filter::parse_expression)
        .transpose()
        .map_err(|error| AppError::Config(format!("RUST_LOG: {error}")))?;

    let cli_parsed = cli_level
        .map(|raw| {
            if raw.contains('=') {
                return Err(AppError::Config(
                    "--log-level takes a bare level; per-target overrides belong in \
                     RUST_LOG or [logging].directives"
                        .to_owned(),
                ));
            }
            filter::parse_level(raw)
                .map_err(|error| AppError::Config(format!("--log-level: {error}")))
        })
        .transpose()?;

    let (global, level_source) = if let Some(level) = cli_parsed {
        (level, Source::Cli)
    } else if let Some(level) = env_parsed.as_ref().and_then(|parsed| parsed.global) {
        (level, Source::RustLog)
    } else {
        (config_level, config_source)
    };

    let (directives, directives_source) = match &env_parsed {
        Some(parsed) => (parsed.directives.clone(), Source::RustLog),
        None => (config_directives, config_source),
    };
    let directive_text = directives
        .iter()
        .map(|directive| format!("{}={}", directive.target(), directive.level()))
        .collect::<Vec<_>>();
    let filter = FilterSpec::new(global, directives);

    let (format, format_source) = match cli_format {
        Some(format) => (format, Source::Cli),
        None => (config.format, config_source),
    };

    let ansi = match config.ansi {
        AnsiMode::Always => true,
        AnsiMode::Never => false,
        AnsiMode::Auto => std::io::stderr().is_terminal(),
    };

    let dir = if config.dir.trim().is_empty() {
        paths.log_dir.clone()
    } else {
        PathBuf::from(config.dir.trim())
    };

    let console = config.console && !tui;
    let console_forced_off = config.console && tui;
    let file = config.file || tui;
    let file_forced_on = !config.file && tui;

    Ok(ResolvedLogging {
        filter,
        level_source,
        directives: directive_text,
        directives_source,
        format,
        format_source,
        ansi_mode: config.ansi,
        ansi,
        console,
        console_forced_off,
        file,
        file_forced_on,
        dir,
        rotation: config.rotate,
        retain_files: config.retain_files,
        max_file_mb: config.max_file_mb,
        max_total_mb: config.max_total_mb,
        ring_lines: config.ring_lines,
        log_prompts: config.log_prompts,
        log_tool_output: config.log_tool_output,
        max_field_bytes: config.max_field_bytes,
        dedup_window_ms: config.dedup_window_ms,
        config_path: loaded.path.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, LoggingConfig};
    use tracing::level_filters::LevelFilter;

    fn paths() -> Paths {
        Paths {
            config_dir: PathBuf::from("/tmp/cfg"),
            config_file: PathBuf::from("/tmp/cfg/config.toml"),
            state_dir: PathBuf::from("/tmp/state"),
            log_dir: PathBuf::from("/tmp/state/logs"),
            db_file: PathBuf::from("/tmp/data/mta.db"),
        }
    }

    fn loaded(level: &str, directives: &[&str]) -> LoadedConfig {
        LoadedConfig {
            config: Config {
                logging: LoggingConfig {
                    level: level.to_owned(),
                    directives: directives.iter().map(|s| (*s).to_owned()).collect(),
                    ..LoggingConfig::default()
                },
            },
            path: PathBuf::from("/tmp/cfg/config.toml"),
            exists: true,
            hash: None,
        }
    }

    #[test]
    fn cli_beats_rust_log_beats_config() {
        let loaded = loaded("warn", &["mta_cli=debug"]);
        let resolved =
            resolve_with_env(&loaded, &paths(), Some("error"), None, false, Some("trace"))
                .expect("resolves");
        assert_eq!(resolved.filter.global(), LevelFilter::ERROR);
        assert_eq!(resolved.level_source, Source::Cli);
        // RUST_LOG, when present, replaces the directive list.
        assert!(resolved.directives.is_empty());
        assert_eq!(resolved.directives_source, Source::RustLog);
    }

    #[test]
    fn rust_log_without_a_level_keeps_the_config_level() {
        let loaded = loaded("warn", &[]);
        let resolved =
            resolve_with_env(&loaded, &paths(), None, None, false, Some("mta_cli=debug"))
                .expect("resolves");
        assert_eq!(resolved.filter.global(), LevelFilter::WARN);
        assert_eq!(resolved.level_source, Source::Config);
        assert_eq!(resolved.directives, vec!["mta_cli=debug"]);
    }

    #[test]
    fn tui_forces_the_file_sink_and_silences_the_console() {
        let mut loaded = loaded("info", &[]);
        loaded.config.logging.console = true;
        loaded.config.logging.file = false;
        let resolved =
            resolve_with_env(&loaded, &paths(), None, None, true, None).expect("resolves");
        assert!(!resolved.console);
        assert!(resolved.console_forced_off);
        assert!(resolved.file);
        assert!(resolved.file_forced_on);
    }

    #[test]
    fn unknown_level_is_an_error() {
        let loaded = loaded("loud", &[]);
        let error = resolve_with_env(&loaded, &paths(), None, None, false, None)
            .expect_err("invalid level");
        assert_eq!(error.exit_code(), 2);
    }

    #[test]
    fn cli_rejects_directive_syntax() {
        let loaded = loaded("info", &[]);
        let error = resolve_with_env(&loaded, &paths(), Some("mta=debug"), None, false, None)
            .expect_err("directive in --log-level");
        assert!(error.to_string().contains("bare level"), "{error}");
    }
}
