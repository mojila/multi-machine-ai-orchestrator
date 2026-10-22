//! End-to-end CLI tests: `--version`, `config`, `doctor`, and exit codes.
//!
//! Every command runs in a sandbox with its own home directory (and XDG overrides on Linux), so
//! tests never touch the developer's real config, state, or logs. Expected paths are computed per
//! platform because `directories::ProjectDirs` uses native locations: XDG on Linux and
//! `~/Library/Application Support` on macOS, where the XDG variables are ignored.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

struct Sandbox {
    dir: TempDir,
}

impl Sandbox {
    fn new() -> Self {
        Self {
            dir: TempDir::new().expect("tempdir"),
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_mta"));
        command
            .env("HOME", self.dir.path().join("home"))
            .env("XDG_CONFIG_HOME", self.dir.path().join("config"))
            .env("XDG_STATE_HOME", self.dir.path().join("state"))
            .env("XDG_DATA_HOME", self.dir.path().join("data"))
            .env_remove("RUST_LOG")
            .env_remove("MTA_CONFIG");
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().expect("run mta")
    }

    fn config_dir(&self) -> PathBuf {
        if cfg!(target_os = "macos") {
            self.dir.path().join("home/Library/Application Support/mta")
        } else {
            self.dir.path().join("config/mta")
        }
    }

    fn state_dir(&self) -> PathBuf {
        if cfg!(target_os = "macos") {
            // `ProjectDirs::state_dir()` is `None` on macOS; the CLI falls back to
            // `<config_dir>/state`.
            self.config_dir().join("state")
        } else {
            self.dir.path().join("state/mta")
        }
    }

    fn config_file(&self) -> PathBuf {
        self.config_dir().join("config.toml")
    }

    fn log_dir(&self) -> PathBuf {
        self.state_dir().join("logs")
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn version_flag_reports_the_package_version() {
    let sandbox = Sandbox::new();
    let output = sandbox.run(&["--version"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stdout(&output).contains(env!("CARGO_PKG_VERSION")),
        "{}",
        stdout(&output)
    );
}

#[test]
fn config_init_creates_a_valid_file_and_refuses_to_clobber() {
    let sandbox = Sandbox::new();

    let first = sandbox.run(&["config", "init"]);
    assert!(first.status.success(), "{}", stderr(&first));
    assert!(sandbox.config_file().exists());
    let body = std::fs::read_to_string(sandbox.config_file()).expect("read config");
    assert!(body.contains("[logging]"));
    assert!(body.contains("level = \"info\""));

    let second = sandbox.run(&["config", "init"]);
    assert_eq!(second.status.code(), Some(2));
    assert!(
        stderr(&second).contains("already exists"),
        "{}",
        stderr(&second)
    );

    let forced = sandbox.run(&["config", "init", "--force"]);
    assert!(forced.status.success(), "{}", stderr(&forced));
}

#[test]
fn config_validate_and_path_are_wired() {
    let sandbox = Sandbox::new();
    assert!(sandbox.run(&["config", "init"]).status.success());

    let validate = sandbox.run(&["config", "validate"]);
    assert!(validate.status.success(), "{}", stderr(&validate));
    assert!(stdout(&validate).contains("ok"));

    let path = sandbox.run(&["config", "path"]);
    assert!(path.status.success());
    assert_eq!(
        stdout(&path).trim(),
        sandbox.config_file().display().to_string()
    );
}

#[test]
fn doctor_works_without_a_config_and_reports_defaults() {
    let sandbox = Sandbox::new();
    let output = sandbox.run(&["doctor", "--logs"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("absent; defaults"), "{text}");
    assert!(text.contains("level         info"), "{text}");
    assert!(text.contains("directory"), "{text}");
}

#[test]
fn doctor_logs_reports_the_resolved_sinks_and_files() {
    let sandbox = Sandbox::new();
    assert!(sandbox.run(&["config", "init"]).status.success());
    let output = sandbox.run(&["--log-level", "debug", "doctor", "--logs"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(
        text.contains("level         debug (from --log-level)"),
        "{text}"
    );
    assert!(
        text.contains(&sandbox.log_dir().display().to_string()),
        "{text}"
    );
    assert!(text.contains("log files:"), "{text}");
}

#[test]
fn doctor_logs_shows_records_written_by_the_run_itself() {
    // The writer thread buffers, so `doctor --logs` must flush before it reads the file back;
    // otherwise it reports an empty "recent records" section for its own run.
    let sandbox = Sandbox::new();
    let output = sandbox.run(&["--log-level", "debug", "doctor", "--logs"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output);
    let recent = text
        .split_once("recent records")
        .map(|(_, rest)| rest)
        .unwrap_or_default();
    assert!(
        recent.contains("diagnostics initialised"),
        "recent records should include this run: {text}"
    );
    assert!(!text.contains("(0 B total)"), "{text}");
}

#[test]
fn invalid_level_exits_two() {
    let sandbox = Sandbox::new();
    let output = sandbox.run(&["--log-level", "loud", "doctor"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).contains("unknown level"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn invalid_config_exits_two_with_a_pointer_to_the_file() {
    let sandbox = Sandbox::new();
    let config_dir = sandbox
        .config_file()
        .parent()
        .expect("parent")
        .to_path_buf();
    std::fs::create_dir_all(&config_dir).expect("mkdir");
    std::fs::write(sandbox.config_file(), "[logging]\nlevel = \"loud\"\n").expect("write");
    let output = sandbox.run(&["doctor"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).contains("config error"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn json_records_parse_and_carry_documented_fields() {
    let sandbox = Sandbox::new();
    let output = sandbox.run(&["--log-level", "debug", "--log-format", "json", "doctor"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let records = stderr(&output);
    let line = records
        .lines()
        .find(|line| line.contains("diagnostics initialised"))
        .unwrap_or_else(|| panic!("no JSON record in {records:?}"));
    let value: serde_json::Value = serde_json::from_str(line).expect("json record");
    assert!(value.get("level").is_some(), "{line}");
    assert!(value.get("target").is_some(), "{line}");
    assert!(value.get("fields").is_some(), "{line}");
}

#[test]
fn config_override_is_honoured() {
    let sandbox = Sandbox::new();
    let custom = sandbox.dir.path().join("elsewhere/custom.toml");
    let output = sandbox
        .command()
        .args(["--config"])
        .arg(&custom)
        .args(["config", "path"])
        .output()
        .expect("run");
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output).trim(), custom.display().to_string());
}

/// Guard against the sandbox pointing at a real path by accident.
#[test]
fn sandbox_paths_are_under_the_tempdir() {
    let sandbox = Sandbox::new();
    let root = sandbox.dir.path();
    assert!(Path::new(&sandbox.config_file()).starts_with(root));
    assert!(Path::new(&sandbox.log_dir()).starts_with(root));
}
