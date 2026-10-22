//! `mta config` — init, validate, path.

use std::fs;
use std::io::Write;

use crate::config::{self, LoadedConfig};
use crate::error::AppError;
use crate::logging::resolve::ResolvedLogging;
use crate::paths::Paths;

/// Writes a commented default config.
pub fn init(paths: &Paths, force: bool) -> Result<(), AppError> {
    if paths.config_file.exists() && !force {
        return Err(AppError::Config(format!(
            "{} already exists; pass --force to overwrite",
            paths.config_file.display()
        )));
    }
    fs::create_dir_all(&paths.config_dir).map_err(|error| {
        AppError::io(
            format!("creating config directory {}", paths.config_dir.display()),
            error,
        )
    })?;

    // Write-then-rename: a crash cannot leave a half-written config behind.
    let temp = paths.config_file.with_extension("toml.tmp");
    fs::write(&temp, config::INIT_TEMPLATE)
        .map_err(|error| AppError::io(format!("writing {}", temp.display()), error))?;
    fs::rename(&temp, &paths.config_file).map_err(|error| {
        let _ = fs::remove_file(&temp);
        AppError::io(format!("installing {}", paths.config_file.display()), error)
    })?;

    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "wrote {}", paths.config_file.display())
        .map_err(|error| AppError::io("writing to stdout", error))
}

/// Reports that the config parsed, validated, and resolved.
pub fn validate(loaded: &LoadedConfig, resolved: &ResolvedLogging) -> Result<(), AppError> {
    let mut stdout = std::io::stdout().lock();
    let source = if loaded.exists {
        loaded.path().display().to_string()
    } else {
        format!("{} (absent; defaults)", loaded.path().display())
    };
    writeln!(stdout, "config: ok ({source})")
        .and_then(|()| {
            writeln!(
                stdout,
                "logging: level {} ({})",
                resolved.filter.global(),
                resolved.level_source.describe(&resolved.config_path)
            )
        })
        .map_err(|error| AppError::io("writing to stdout", error))
}

/// Prints the config file path.
pub fn path(paths: &Paths) -> Result<(), AppError> {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{}", paths.config_file.display())
        .map_err(|error| AppError::io("writing to stdout", error))
}
