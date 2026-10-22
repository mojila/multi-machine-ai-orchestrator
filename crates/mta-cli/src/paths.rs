//! XDG paths, with `--config` overriding the config file only.

use std::path::{Path, PathBuf};

use directories::ProjectDirs;

use crate::error::AppError;

/// Resolved filesystem locations.
#[derive(Debug, Clone)]
pub struct Paths {
    /// Directory holding the config file.
    pub config_dir: PathBuf,
    /// The config file itself.
    pub config_file: PathBuf,
    /// `$XDG_STATE_HOME/mta` (logs and crash markers live below it).
    pub state_dir: PathBuf,
    /// `$XDG_STATE_HOME/mta/logs`.
    pub log_dir: PathBuf,
    /// The SQLite database path.
    pub db_file: PathBuf,
}

impl Paths {
    /// Resolves the standard paths, honouring `--config`.
    pub fn discover(config_override: Option<&Path>) -> Result<Self, AppError> {
        let dirs = ProjectDirs::from("", "", "mta").ok_or_else(|| {
            AppError::Config(
                "could not determine XDG config/state/data directories; \
                 set HOME (or XDG_CONFIG_HOME/XDG_STATE_HOME/XDG_DATA_HOME) and retry"
                    .to_owned(),
            )
        })?;

        let config_file = config_override
            .map_or_else(|| dirs.config_dir().join("config.toml"), Path::to_path_buf);
        let config_dir = config_file
            .parent()
            .map_or_else(|| dirs.config_dir().to_path_buf(), Path::to_path_buf);

        let state_dir = dirs
            .state_dir()
            .map_or_else(|| dirs.config_dir().join("state"), Path::to_path_buf);
        let data_dir = dirs.data_dir().to_path_buf();

        Ok(Self {
            log_dir: state_dir.join("logs"),
            db_file: data_dir.join("mta.db"),
            config_dir,
            config_file,
            state_dir,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_override_changes_only_the_config_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let custom = temp.path().join("x/config.toml");
        let paths = Paths::discover(Some(&custom)).expect("paths");
        assert_eq!(paths.config_file, custom);
        assert_eq!(paths.config_dir, temp.path().join("x"));
        assert!(paths.db_file.ends_with("mta.db"));
        assert!(paths.log_dir.ends_with("logs"));
    }
}
