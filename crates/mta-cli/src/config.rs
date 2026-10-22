//! Configuration file types, defaults, validation, and loading.
//!
//! M0 implements the `[logging]` section (docs/observability.md §3). Later milestones add the
//! other sections; unknown keys are rejected so a typo cannot silently disable a setting.

use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::AppError;
use crate::logging::filter;
use crate::paths::Paths;

/// Top-level configuration.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Diagnostics settings.
    pub logging: LoggingConfig,
}

/// Diagnostics settings (docs/observability.md §3).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct LoggingConfig {
    /// Global level: `error | warn | info | debug | trace | off`.
    pub level: String,
    /// Per-target overrides, e.g. `["mta_engine::effects=debug", "hyper=warn"]`.
    pub directives: Vec<String>,
    /// Record format written to both sinks.
    pub format: LogFormat,
    /// ANSI colour policy for the console sink.
    pub ansi: AnsiMode,
    /// Enable the stderr sink (disabled automatically in TUI mode).
    pub console: bool,
    /// Enable the rotating file sink (forced on in TUI mode).
    pub file: bool,
    /// Log directory; empty means `$XDG_STATE_HOME/mta/logs`.
    pub dir: String,
    /// Rotation cadence.
    pub rotate: Rotation,
    /// Files kept per rotation series.
    pub retain_files: usize,
    /// Rotate rather than grow a file past this size.
    pub max_file_mb: u64,
    /// Hard cap across the directory; the oldest files are pruned first.
    pub max_total_mb: u64,
    /// In-process ring size for the TUI overlay (not persisted; lands in M2).
    pub ring_lines: usize,
    /// Include prompt/response text in records.
    pub log_prompts: bool,
    /// Include tool stdout/stderr bodies in records.
    pub log_tool_output: bool,
    /// Truncate any recorded field at this many bytes.
    pub max_field_bytes: usize,
    /// Collapse identical repeated records inside this window.
    pub dedup_window_ms: u64,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_owned(),
            directives: Vec::new(),
            format: LogFormat::Human,
            ansi: AnsiMode::Auto,
            console: true,
            file: true,
            dir: String::new(),
            rotate: Rotation::Daily,
            retain_files: 7,
            max_file_mb: 16,
            max_total_mb: 128,
            ring_lines: 2000,
            log_prompts: false,
            log_tool_output: false,
            max_field_bytes: 2048,
            dedup_window_ms: 1000,
        }
    }
}

/// Record format.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    /// Human-readable lines.
    #[default]
    Human,
    /// One JSON object per record.
    Json,
}

impl fmt::Display for LogFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Human => f.write_str("human"),
            Self::Json => f.write_str("json"),
        }
    }
}

/// ANSI colour policy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AnsiMode {
    /// Colour when stderr is a terminal.
    #[default]
    Auto,
    /// Always emit escapes.
    Always,
    /// Never emit escapes.
    Never,
}

impl fmt::Display for AnsiMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => f.write_str("auto"),
            Self::Always => f.write_str("always"),
            Self::Never => f.write_str("never"),
        }
    }
}

/// File rotation cadence.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Rotation {
    /// One file per UTC day.
    #[default]
    Daily,
    /// One file per UTC hour.
    Hourly,
    /// A single `mta.log`.
    Never,
}

impl fmt::Display for Rotation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Daily => f.write_str("daily"),
            Self::Hourly => f.write_str("hourly"),
            Self::Never => f.write_str("never"),
        }
    }
}

impl LoggingConfig {
    /// Validates every value, returning all problems rather than only the first.
    pub fn validate(&self) -> Result<(), AppError> {
        let mut problems = Vec::new();

        if let Err(error) = filter::parse_level(&self.level) {
            problems.push(format!("logging.level: {error}"));
        }
        if let Err(error) = filter::parse_directives(&self.directives) {
            problems.push(format!("logging.directives: {error}"));
        }
        if self.retain_files == 0 {
            problems.push("logging.retain_files: must be at least 1".to_owned());
        }
        if self.max_file_mb == 0 {
            problems.push("logging.max_file_mb: must be at least 1".to_owned());
        }
        if self.max_total_mb < self.max_file_mb {
            problems.push(format!(
                "logging.max_total_mb: {} is smaller than max_file_mb {}",
                self.max_total_mb, self.max_file_mb
            ));
        }
        if self.max_field_bytes == 0 {
            problems.push("logging.max_field_bytes: must be at least 1".to_owned());
        }
        if self.dedup_window_ms > 60_000 {
            problems.push("logging.dedup_window_ms: must be at most 60000".to_owned());
        }

        if problems.is_empty() {
            Ok(())
        } else {
            Err(AppError::Config(format!(
                "invalid {}:\n  - {}",
                "config",
                problems.join("\n  - ")
            )))
        }
    }
}

/// A config file that has been read (or defaults, when absent).
#[derive(Debug, Clone)]
pub struct LoadedConfig {
    /// The parsed config.
    pub config: Config,
    /// The path that was consulted.
    pub path: std::path::PathBuf,
    /// Whether the file existed.
    pub exists: bool,
    /// SHA-256 of the raw bytes, when the file exists.
    pub hash: Option<String>,
}

impl LoadedConfig {
    /// Loads and validates the config file.
    pub fn load(paths: &Paths) -> Result<Self, AppError> {
        match std::fs::read(&paths.config_file) {
            Ok(bytes) => {
                let text = String::from_utf8(bytes.clone()).map_err(|_| {
                    AppError::Config(format!(
                        "{} is not valid UTF-8",
                        paths.config_file.display()
                    ))
                })?;
                let config: Config = toml::from_str(&text).map_err(|error| {
                    AppError::Config(format!("{}: {error}", paths.config_file.display()))
                })?;
                config.logging.validate()?;
                Ok(Self {
                    config,
                    path: paths.config_file.clone(),
                    exists: true,
                    hash: Some(sha256_hex(&bytes)),
                })
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self {
                config: Config::default(),
                path: paths.config_file.clone(),
                exists: false,
                hash: None,
            }),
            Err(error) => Err(AppError::io(
                format!("reading {}", paths.config_file.display()),
                error,
            )),
        }
    }

    /// The path, for display in `doctor`.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// The template written by `mta config init`, with the documentation inline.
pub const INIT_TEMPLATE: &str = r#"# mta configuration — docs/observability.md §3
#
# Every value below is the default. Comments are preserved by future config edits
# (the engine's ConfigEditor), so keep notes next to the setting they explain.

[logging]
level = "info"              # error | warn | info | debug | trace | off
directives = []             # ["mta_engine::effects=debug", "hyper=warn"]
format = "human"            # human | json
ansi = "auto"               # auto | always | never
console = true              # stderr sink (auto-disabled while the TUI owns the screen)
file = true                 # rotating file sink (forced on in TUI mode)
dir = ""                    # empty = $XDG_STATE_HOME/mta/logs
rotate = "daily"            # daily | hourly | never
retain_files = 7            # files kept per rotation series
max_file_mb = 16            # rotate/prune rather than grow past this
max_total_mb = 128          # hard cap; oldest files are pruned first
ring_lines = 2000           # in-process TUI overlay buffer (not persisted)
log_prompts = false         # include prompt/response text
log_tool_output = false     # include tool stdout/stderr bodies
max_field_bytes = 2048      # truncate any recorded field
dedup_window_ms = 1000      # collapse identical repeated records
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid_and_matches_the_template() {
        let from_template: Config = toml::from_str(INIT_TEMPLATE).expect("template parses");
        assert_eq!(
            toml::to_string(&Config::default()).expect("serialize"),
            toml::to_string(&from_template).expect("serialize")
        );
        from_template.logging.validate().expect("valid");
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let error =
            toml::from_str::<Config>("[logging]\nlevl = \"debug\"\n").expect_err("unknown key");
        assert!(error.to_string().contains("levl"), "{error}");
    }

    #[test]
    fn invalid_numbers_are_all_reported() {
        let config = Config {
            logging: LoggingConfig {
                retain_files: 0,
                max_file_mb: 0,
                max_total_mb: 0,
                ..LoggingConfig::default()
            },
        };
        let error = config.logging.validate().expect_err("invalid");
        let text = error.to_string();
        assert!(text.contains("retain_files"), "{text}");
        assert!(text.contains("max_file_mb"), "{text}");
    }
}
