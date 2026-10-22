//! The rotating file sink and its bounded, lossy write queue.
//!
//! Records are handed to one writer thread through a bounded channel. A full channel drops the
//! record and counts it rather than blocking the engine (docs/observability.md §9). The sink
//! exposes [`FileSink::flush`] so the panic hook can get the queue onto disk before the process
//! dies; `tracing-appender`'s own guard cannot do that.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime};

use tracing_appender::rolling::RollingFileAppender;
use tracing_subscriber::fmt::writer::MakeWriter;

use crate::config::Rotation as ConfiguredRotation;

/// Queue capacity in records. Bounded on purpose: diagnostics must never grow without limit.
const QUEUE_CAPACITY: usize = 4096;

enum Msg {
    Bytes(Vec<u8>),
    Flush(SyncSender<()>),
    Shutdown,
}

/// A shared file-sink handle.
#[derive(Debug, Clone)]
pub struct FileSink {
    tx: SyncSender<Msg>,
    dropped: Arc<AtomicU64>,
    write_error: Arc<Mutex<Option<String>>>,
    join: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl FileSink {
    /// Starts the writer thread.
    pub fn new(appender: RollingFileAppender) -> io::Result<Self> {
        let (tx, rx) = sync_channel(QUEUE_CAPACITY);
        let dropped = Arc::new(AtomicU64::new(0));
        let write_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

        let thread_error = Arc::clone(&write_error);
        let join = thread::Builder::new()
            .name("mta-log-writer".to_owned())
            .spawn(move || run_writer(appender, rx, &thread_error))?;

        Ok(Self {
            tx,
            dropped,
            write_error,
            join: Arc::new(Mutex::new(Some(join))),
        })
    }

    /// A handle for `fmt::Layer::with_writer`.
    #[must_use]
    pub fn writer(&self) -> SinkWriter {
        SinkWriter {
            tx: self.tx.clone(),
            dropped: Arc::clone(&self.dropped),
        }
    }

    /// Records dropped because the queue was full.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Flushes everything queued so far and waits up to `timeout`.
    pub fn flush(&self, timeout: Duration) -> io::Result<()> {
        if let Some(error) = self.write_error.lock().ok().and_then(|slot| slot.clone()) {
            return Err(io::Error::other(error));
        }
        let (ack_tx, ack_rx) = sync_channel(1);
        self.tx
            .send(Msg::Flush(ack_tx))
            .map_err(|_| io::Error::other("log writer thread is gone"))?;
        match ack_rx.recv_timeout(timeout) {
            Ok(()) => Ok(()),
            Err(RecvTimeoutError::Timeout) => Err(io::Error::other("log flush timed out")),
            Err(RecvTimeoutError::Disconnected) => {
                Err(io::Error::other("log writer thread is gone"))
            }
        }
    }
}

impl Drop for FileSink {
    fn drop(&mut self) {
        // Shut the worker down so buffered records reach the file before the process exits.
        let _ = self.tx.send(Msg::Shutdown);
        if let Ok(mut slot) = self.join.lock()
            && let Some(handle) = slot.take()
        {
            let _ = handle.join();
        }
    }
}

fn run_writer(
    mut appender: RollingFileAppender,
    rx: Receiver<Msg>,
    write_error: &Mutex<Option<String>>,
) {
    while let Ok(msg) = rx.recv() {
        match msg {
            Msg::Bytes(bytes) => {
                if let Err(error) = appender.write_all(&bytes)
                    && let Ok(mut slot) = write_error.lock()
                {
                    *slot = Some(error.to_string());
                }
            }
            Msg::Flush(ack) => {
                let result = appender.flush();
                if let Err(error) = result
                    && let Ok(mut slot) = write_error.lock()
                {
                    *slot = Some(error.to_string());
                }
                let _ = ack.send(());
            }
            Msg::Shutdown => break,
        }
    }
    let _ = appender.flush();
}

/// The `MakeWriter` given to `tracing-subscriber`.
#[derive(Debug, Clone)]
pub struct SinkWriter {
    tx: SyncSender<Msg>,
    dropped: Arc<AtomicU64>,
}

impl Write for SinkWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self.tx.try_send(Msg::Bytes(buf.to_vec())) {
            Ok(()) => Ok(buf.len()),
            Err(TrySendError::Full(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                Ok(buf.len())
            }
            Err(TrySendError::Disconnected(_)) => Err(io::Error::other("log sink closed")),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for SinkWriter {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Builds the configured rolling appender.
pub fn build_appender(
    dir: &Path,
    rotation: ConfiguredRotation,
    retain_files: usize,
) -> io::Result<RollingFileAppender> {
    let rotation = match rotation {
        ConfiguredRotation::Daily => tracing_appender::rolling::Rotation::DAILY,
        ConfiguredRotation::Hourly => tracing_appender::rolling::Rotation::HOURLY,
        ConfiguredRotation::Never => tracing_appender::rolling::Rotation::NEVER,
    };
    RollingFileAppender::builder()
        .rotation(rotation)
        .filename_prefix("mta")
        .filename_suffix("log")
        .max_log_files(retain_files)
        .build(dir)
        .map_err(io::Error::other)
}

/// What a prune pass removed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PruneReport {
    /// Number of files removed.
    pub removed: usize,
    /// Bytes reclaimed.
    pub bytes: u64,
}

/// Enforces `retain_files`, `max_file_mb`, and `max_total_mb` over the log directory.
///
/// Called at startup and before the event loop; a failure is reported as a `warn` and never
/// affects a run (losing diagnostics is harmless). The oldest files go first.
pub fn prune(
    dir: &Path,
    retain_files: usize,
    max_file_mb: u64,
    max_total_mb: u64,
) -> io::Result<PruneReport> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !is_log_file(&path) || !entry.file_type()?.is_file() {
            continue;
        }
        let size = entry.metadata()?.len();
        let modified = entry
            .metadata()?
            .modified()
            .unwrap_or(SystemTime::UNIX_EPOCH);
        files.push((path, size, modified));
    }
    files.sort_by(|a, b| a.2.cmp(&b.2).then_with(|| a.0.cmp(&b.0)));

    let mut report = PruneReport::default();
    let max_file = max_file_mb.saturating_mul(1024 * 1024);

    // An oversized old file is removed regardless of the count: the cap is hard.
    while files.first().is_some_and(|(_, size, _)| *size > max_file) {
        let (path, size, _) = files.remove(0);
        if remove(&path)? {
            report.removed += 1;
            report.bytes += size;
        }
    }

    let mut total: u64 = files.iter().map(|(_, size, _)| size).sum();
    let max_total = max_total_mb.saturating_mul(1024 * 1024);
    while files.len() > retain_files.max(1) || total > max_total {
        if files.is_empty() {
            break;
        }
        let (path, size, _) = files.remove(0);
        if remove(&path)? {
            report.removed += 1;
            report.bytes += size;
            total = total.saturating_sub(size);
        }
    }

    Ok(report)
}

fn remove(path: &Path) -> io::Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn is_log_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    name == "mta.log" || (name.starts_with("mta.") && name.ends_with(".log"))
}

/// Lists the log files, oldest first, with sizes.
pub fn inventory(dir: &Path) -> io::Result<Vec<(PathBuf, u64, SystemTime)>> {
    let mut files = Vec::new();
    if !dir.exists() {
        return Ok(files);
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if !is_log_file(&entry.path()) {
            continue;
        }
        let metadata = entry.metadata()?;
        files.push((
            entry.path(),
            metadata.len(),
            metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
        ));
    }
    files.sort_by(|a, b| a.2.cmp(&b.2));
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_flushes_to_a_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let appender = build_appender(dir.path(), ConfiguredRotation::Never, 7).expect("appender");
        let sink = FileSink::new(appender).expect("sink");
        sink.writer()
            .write_all(b"hello diagnostics\n")
            .expect("write");
        sink.flush(Duration::from_secs(2)).expect("flush");

        let contents = fs::read_to_string(dir.path().join("mta.log")).expect("read log");
        assert_eq!(contents, "hello diagnostics\n");
    }

    #[test]
    fn prune_keeps_the_newest_n_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        for day in 1..=5 {
            fs::write(
                dir.path().join(format!("mta.2026-01-0{day}.log")),
                b"one record\n",
            )
            .expect("write");
        }
        let report = prune(dir.path(), 2, 64, 1024).expect("prune");
        assert_eq!(report.removed, 3);
        let remaining: Vec<_> = inventory(dir.path()).expect("inventory");
        assert_eq!(remaining.len(), 2);
    }

    #[test]
    fn prune_enforces_the_total_cap() {
        let dir = tempfile::tempdir().expect("tempdir");
        for day in 1..=3 {
            fs::write(
                dir.path().join(format!("mta.2026-02-0{day}.log")),
                vec![b'x'; 700 * 1024],
            )
            .expect("write");
        }
        // 3 x ~0.7 MiB exceeds a 1 MiB total cap.
        let report = prune(dir.path(), 7, 16, 1).expect("prune");
        assert!(report.removed >= 1, "{report:?}");
        assert!(report.bytes >= 700 * 1024);
    }

    #[test]
    fn ignores_non_log_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("notes.txt"), b"keep me").expect("write");
        let report = prune(dir.path(), 1, 1, 1).expect("prune");
        assert_eq!(report.removed, 0);
        assert!(dir.path().join("notes.txt").exists());
    }
}
