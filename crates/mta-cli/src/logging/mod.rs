//! Diagnostics setup: filter, sinks, crash hook, and the test-facing contract.
//!
//! `mta` keeps two records (docs/observability.md): the durable event log (M1) and this
//! ephemeral diagnostic log. It is never replayed and never authoritative. `mta-domain` cannot
//! depend on `tracing`, so deciders cannot log.

pub mod crash;
pub mod filter;
pub mod resolve;
pub mod sink;

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tracing::level_filters::LevelFilter;
use tracing_subscriber::Layer;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::config::LogFormat;
use crate::error::AppError;
use crate::logging::resolve::ResolvedLogging;

type BoxedLayer = Box<dyn Layer<tracing_subscriber::Registry> + Send + Sync>;

const FLUSH_TIMEOUT: Duration = Duration::from_secs(2);

/// The installed diagnostics stack. Keep it alive for the whole process.
#[derive(Debug)]
pub struct LoggingGuard {
    sink: Option<Arc<sink::FileSink>>,
    /// `flush` may run more than once (a command that inspects the logs, then process exit); the
    /// drop warning is worth saying once, not once per flush.
    warned_drops: AtomicBool,
}

impl LoggingGuard {
    /// Flushes the file sink, waiting briefly. Errors are deliberately ignored here; the panic
    /// path must not panic again.
    pub fn flush(&self) {
        if let Some(sink) = &self.sink {
            let dropped = sink.dropped();
            if dropped > 0 && !self.warned_drops.swap(true, Ordering::Relaxed) {
                tracing::warn!(
                    log.dropped_records = dropped,
                    "diagnostic records were dropped because the writer queue was full"
                );
            }
            let _ = sink.flush(FLUSH_TIMEOUT);
        }
    }

    /// A closure the panic hook can call before writing its marker.
    #[must_use]
    pub fn flusher(&self) -> Arc<dyn Fn() + Send + Sync> {
        let sink = self.sink.clone();
        Arc::new(move || {
            if let Some(sink) = &sink {
                let _ = sink.flush(FLUSH_TIMEOUT);
            }
        })
    }
}

fn layer_filter(
    spec: &filter::FilterSpec,
) -> impl for<'a, 'b> Fn(&'a tracing::Metadata<'b>) -> bool + Clone + Send + Sync + 'static {
    let spec = spec.clone();
    move |metadata: &tracing::Metadata<'_>| {
        spec.enabled(
            metadata.target(),
            LevelFilter::from_level(*metadata.level()),
        )
    }
}

/// Installs the global subscriber.
///
/// The console sink writes to stderr only; the TUI silences it and forces the file sink on. A
/// second call fails because the global subscriber can only be installed once.
pub fn init(resolved: &ResolvedLogging) -> Result<LoggingGuard, AppError> {
    let mut layers: Vec<BoxedLayer> = Vec::new();

    if resolved.console {
        let layer: BoxedLayer = match resolved.format {
            LogFormat::Human => fmt::layer()
                .with_writer(std::io::stderr)
                .with_ansi(resolved.ansi)
                .boxed(),
            LogFormat::Json => fmt::layer()
                .json()
                .with_writer(std::io::stderr)
                .with_ansi(resolved.ansi)
                .boxed(),
        };
        let layer: BoxedLayer = layer
            .with_filter(tracing_subscriber::filter::filter_fn(layer_filter(
                &resolved.filter,
            )))
            .boxed();
        layers.push(layer);
    }

    let mut file_sink = None;
    if resolved.file {
        fs::create_dir_all(&resolved.dir).map_err(|error| {
            AppError::io(
                format!("creating log directory {}", resolved.dir.display()),
                error,
            )
        })?;

        let prune = sink::prune(
            &resolved.dir,
            resolved.retain_files,
            resolved.max_file_mb,
            resolved.max_total_mb,
        );

        let appender =
            sink::build_appender(&resolved.dir, resolved.rotation, resolved.retain_files).map_err(
                |error| {
                    AppError::io(
                        format!("opening log file in {}", resolved.dir.display()),
                        error,
                    )
                },
            )?;
        let sink =
            Arc::new(sink::FileSink::new(appender).map_err(|error| {
                AppError::io("starting the log writer thread".to_owned(), error)
            })?);
        let writer = sink.writer();

        let layer: BoxedLayer = match resolved.format {
            LogFormat::Human => fmt::layer().with_writer(writer).with_ansi(false).boxed(),
            LogFormat::Json => fmt::layer()
                .json()
                .with_writer(writer)
                .with_ansi(false)
                .boxed(),
        };
        let layer: BoxedLayer = layer
            .with_filter(tracing_subscriber::filter::filter_fn(layer_filter(
                &resolved.filter,
            )))
            .boxed();
        layers.push(layer);
        file_sink = Some(sink);

        if let Ok(report) = prune
            && report.removed > 0
        {
            tracing::warn!(
                log.directory = %resolved.dir.display(),
                log.removed = report.removed,
                log.bytes = report.bytes,
                "pruned old diagnostic logs to respect the size budget"
            );
        }
    }

    let subscriber = tracing_subscriber::registry().with(layers);
    subscriber
        .try_init()
        .map_err(|error| AppError::Logging(format!("installing the subscriber: {error}")))?;

    if resolved.file_forced_on {
        tracing::warn!(
            "file logging forced on in TUI mode; `console = true, file = false` is not supported"
        );
    }
    if resolved.console_forced_off {
        tracing::debug!("console sink disabled while the TUI owns the terminal");
    }

    Ok(LoggingGuard {
        sink: file_sink,
        warned_drops: AtomicBool::new(false),
    })
}

/// The current log directory's files, for `doctor`.
pub fn log_files(
    dir: &std::path::Path,
) -> std::io::Result<Vec<(PathBuf, u64, std::time::SystemTime)>> {
    sink::inventory(dir)
}
