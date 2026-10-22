//! Panic hook and crash markers.
//!
//! The hook records the panic at `error`, flushes the file sink, writes a small
//! `crash-<unix-ms>.json` marker (no secrets, no buffer dump), restores the terminal, and then
//! runs the previous hook so the message still reaches stderr. The next startup reports and
//! deletes any marker once (docs/observability.md §2).

use std::fs;
use std::io::{self, Write};
use std::panic::{self, PanicHookInfo};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Everything the marker needs that must be captured before the panic.
#[derive(Debug, Clone)]
pub struct CrashContext {
    /// `CARGO_PKG_VERSION`.
    pub version: String,
    /// SHA-256 of the config file, when one exists.
    pub config_hash: Option<String>,
    /// The file sink's current path pattern, for the report.
    pub log_path: Option<PathBuf>,
    /// Directory the marker is written to.
    pub crash_dir: PathBuf,
    /// Maximum bytes of panic message recorded.
    pub max_field_bytes: usize,
}

/// The on-disk marker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrashMarker {
    /// `mta` version.
    pub version: String,
    /// Unix epoch milliseconds.
    pub at_ms: u128,
    /// Config hash at the time of the crash.
    pub config_hash: Option<String>,
    /// Log path or directory.
    pub log_path: Option<String>,
    /// The panic message, truncated.
    pub message: String,
    /// Panic location, when available.
    pub location: Option<String>,
}

/// Installs the wrapped panic hook.
///
/// `flush` is called before the marker is written and `restore` before the previous hook prints,
/// so the terminal is usable again when the message appears.
pub fn install<F>(context: CrashContext, flush: F, restore: fn())
where
    F: Fn() + Send + Sync + 'static,
{
    let flush = Arc::new(flush);
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info: &PanicHookInfo<'_>| {
        let message = panic_message(info, context.max_field_bytes);
        tracing::error!(
            panic.message = %message,
            panic.location = info.location().map(ToString::to_string).as_deref().unwrap_or("unknown"),
            "panic"
        );
        flush();
        write_marker(&context, &message, info);
        restore();
        previous(info);
    }));
}

fn panic_message(info: &PanicHookInfo<'_>, max_bytes: usize) -> String {
    let payload = info.payload();
    let text = payload
        .downcast_ref::<&str>()
        .map(|text| (*text).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "<non-string panic payload>".to_owned());
    truncate(&text, max_bytes)
}

fn truncate(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_owned();
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… (+{} bytes)", &text[..end], text.len() - end)
}

fn write_marker(context: &CrashContext, message: &str, info: &PanicHookInfo<'_>) {
    let marker = CrashMarker {
        version: context.version.clone(),
        at_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_millis()),
        config_hash: context.config_hash.clone(),
        log_path: context
            .log_path
            .as_ref()
            .map(|path| path.display().to_string()),
        message: message.to_owned(),
        location: info.location().map(ToString::to_string),
    };
    if fs::create_dir_all(&context.crash_dir).is_err() {
        return;
    }
    let path = context
        .crash_dir
        .join(format!("crash-{}.json", marker.at_ms));
    let Ok(json) = serde_json::to_vec_pretty(&marker) else {
        return;
    };
    let _ = fs::write(path, json);
}

/// Reports and removes crash markers left by previous sessions.
///
/// Returns the number reported. Records are written through `tracing` (file always, stderr when
/// the console sink is on).
pub fn report_and_clear(dir: &Path, console: bool) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    let mut reported = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !(name.starts_with("crash-") && name.ends_with(".json")) {
            continue;
        }
        let text = fs::read_to_string(&path).unwrap_or_default();
        match serde_json::from_str::<CrashMarker>(&text) {
            Ok(marker) => {
                tracing::warn!(
                    crash.marker = %path.display(),
                    crash.at_ms = marker.at_ms,
                    crash.version = %marker.version,
                    "previous session ended abnormally; the diagnostic log has the details"
                );
                if console {
                    // `report_and_clear` runs before the alternate screen; this single line is a
                    // terminal write by design and uses an explicit writer (no `eprintln!`).
                    let mut stderr = io::stderr().lock();
                    let _ = writeln!(
                        stderr,
                        "mta: previous session ended abnormally at {} ({}); log: {}",
                        marker.at_ms,
                        marker.message,
                        marker.log_path.as_deref().unwrap_or("<none>")
                    );
                }
            }
            Err(_) => {
                tracing::warn!(
                    crash.marker = %path.display(),
                    "unreadable crash marker from a previous session"
                );
            }
        }
        let _ = fs::remove_file(&path);
        reported += 1;
    }
    reported
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(dir: &Path) -> CrashContext {
        CrashContext {
            version: "0.0.0-test".to_owned(),
            config_hash: Some("abc".to_owned()),
            log_path: Some(dir.join("mta.log")),
            crash_dir: dir.to_path_buf(),
            max_field_bytes: 32,
        }
    }

    #[test]
    fn truncation_is_char_safe_and_labelled() {
        let text = "é".repeat(40);
        let truncated = truncate(&text, 10);
        assert!(truncated.ends_with("bytes)"), "{truncated}");
        assert!(truncated.is_char_boundary(truncated.len()));
    }

    #[test]
    fn report_removes_the_marker() {
        let dir = tempfile::tempdir().expect("tempdir");
        let marker = CrashMarker {
            version: "0.0.0-test".to_owned(),
            at_ms: 123,
            config_hash: None,
            log_path: None,
            message: "boom".to_owned(),
            location: None,
        };
        fs::write(
            dir.path().join("crash-123.json"),
            serde_json::to_vec(&marker).expect("json"),
        )
        .expect("write");
        assert_eq!(report_and_clear(dir.path(), false), 1);
        assert!(!dir.path().join("crash-123.json").exists());
    }

    #[test]
    fn unreadable_markers_still_clear_and_do_not_panic() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("crash-1.json"), b"not json").expect("write");
        assert_eq!(report_and_clear(dir.path(), false), 1);
        assert!(!dir.path().join("crash-1.json").exists());
    }

    #[test]
    fn marker_contains_no_secret_value() {
        let dir = tempfile::tempdir().expect("tempdir");
        let context = context(dir.path());
        // `write_marker` only records the panic message; a secret passed as a field is never
        // handed to it. Guard the shape of the file anyway.
        let marker = CrashMarker {
            version: context.version.clone(),
            at_ms: 1,
            config_hash: context.config_hash.clone(),
            log_path: None,
            message: "boom".to_owned(),
            location: None,
        };
        let json = serde_json::to_string(&marker).expect("json");
        assert!(!json.contains("sk-"));
        assert!(json.contains("0.0.0-test"));
    }

    /// Child half of the panic smoke test. Without `MTA_PANIC_DIR` it is a no-op so the normal
    /// suite ignores it.
    #[test]
    fn panic_child() {
        let Ok(dir) = std::env::var("MTA_PANIC_DIR") else {
            return;
        };
        let dir = PathBuf::from(dir);
        let context = CrashContext {
            version: "0.0.0-test".to_owned(),
            config_hash: Some("abc".to_owned()),
            log_path: Some(dir.join("mta.log")),
            crash_dir: dir.clone(),
            max_field_bytes: 64,
        };
        install(
            context,
            move || {
                let _ = fs::write(dir.join("flushed"), b"ok");
            },
            restore_marker,
        );
        panic!("boom for the panic smoke test");
    }

    fn restore_marker() {
        if let Ok(path) = std::env::var("MTA_PANIC_RESTORE") {
            let _ = fs::write(path, b"restored");
        }
    }

    #[test]
    fn panic_hook_flushes_writes_a_marker_and_restores() {
        let dir = tempfile::tempdir().expect("tempdir");
        let restore = dir.path().join("restored");
        let status = std::process::Command::new(std::env::current_exe().expect("current exe"))
            .args([
                "--exact",
                "logging::crash::tests::panic_child",
                "--nocapture",
            ])
            .env("MTA_PANIC_DIR", dir.path())
            .env("MTA_PANIC_RESTORE", &restore)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("spawn child test");

        assert!(!status.success(), "the child must have panicked");
        assert!(dir.path().join("flushed").exists(), "flush hook not called");
        assert!(restore.exists(), "terminal restore hook not called");

        let marker = fs::read_dir(dir.path())
            .expect("read dir")
            .flatten()
            .find(|entry| entry.file_name().to_string_lossy().starts_with("crash-"))
            .expect("crash marker");
        let body = fs::read_to_string(marker.path()).expect("read marker");
        assert!(body.contains("boom"), "{body}");
        assert!(body.contains("0.0.0-test"), "{body}");
    }
}
