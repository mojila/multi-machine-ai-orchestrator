//! In-memory capture of `tracing` records for tests.
//!
//! ```
//! use mta_testkit::capture::Capture;
//! use tracing::level_filters::LevelFilter;
//!
//! let capture = Capture::start(LevelFilter::TRACE);
//! tracing::info!(target: "mta_testkit::doctest", answer = 42, "hello");
//! assert!(capture.records().iter().any(|record| record.contains("hello")));
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use tracing::field::{Field, Visit};
use tracing::level_filters::LevelFilter;
use tracing::{Event, Level, Metadata, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

/// One captured record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedRecord {
    /// The record level.
    pub level: Level,
    /// The target (`module_path!()` by default).
    pub target: String,
    /// Recorded fields, including `message`.
    pub fields: BTreeMap<String, String>,
}

impl CapturedRecord {
    /// The formatted message, when the record has one.
    #[must_use]
    pub fn message(&self) -> Option<&str> {
        self.fields.get("message").map(String::as_str)
    }

    /// Renders the record as one searchable string.
    #[must_use]
    pub fn text(&self) -> String {
        let fields = self
            .fields
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join(" ");
        format!("{} {} {fields}", self.level, self.target)
    }

    /// Whether any part of the record contains `needle`.
    #[must_use]
    pub fn contains(&self, needle: &str) -> bool {
        self.text().contains(needle)
    }
}

/// An installed capture. Records are captured on the current thread while it is alive.
#[derive(Debug)]
pub struct Capture {
    records: Arc<Mutex<Vec<CapturedRecord>>>,
    _guard: tracing::subscriber::DefaultGuard,
}

impl Capture {
    /// Installs a thread-local subscriber that captures records up to `max_level`.
    #[must_use]
    pub fn start(max_level: LevelFilter) -> Self {
        let records = Arc::new(Mutex::new(Vec::new()));
        let layer = CollectLayer {
            max_level,
            records: Arc::clone(&records),
        };
        let subscriber = tracing_subscriber::registry().with(layer);
        let guard = tracing::subscriber::set_default(subscriber);
        Self {
            records,
            _guard: guard,
        }
    }

    /// A snapshot of the records captured so far.
    #[must_use]
    pub fn records(&self) -> Vec<CapturedRecord> {
        self.records.lock().map_or_else(
            |poisoned| poisoned.into_inner().clone(),
            |records| records.clone(),
        )
    }

    /// Whether any record contains the given text. This is how the redaction property test
    /// asserts that a secret never reached a record (docs/observability.md §5).
    #[must_use]
    pub fn contains(&self, needle: &str) -> bool {
        self.records().iter().any(|record| record.contains(needle))
    }
}

struct CollectLayer {
    max_level: LevelFilter,
    records: Arc<Mutex<Vec<CapturedRecord>>>,
}

impl<S: Subscriber> Layer<S> for CollectLayer {
    fn enabled(&self, metadata: &Metadata<'_>, _ctx: Context<'_, S>) -> bool {
        LevelFilter::from_level(*metadata.level()) <= self.max_level
    }

    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let metadata = event.metadata();
        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);
        if let Ok(mut records) = self.records.lock() {
            records.push(CapturedRecord {
                level: *metadata.level(),
                target: metadata.target().to_owned(),
                fields: visitor.fields,
            });
        }
    }
}

#[derive(Default)]
struct FieldVisitor {
    fields: BTreeMap<String, String>,
}

impl Visit for FieldVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.fields
            .insert(field.name().to_owned(), format!("{value:?}"));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.fields
            .insert(field.name().to_owned(), value.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_level_target_and_fields() {
        let capture = Capture::start(LevelFilter::DEBUG);
        tracing::debug!(target: "mta_testkit::tests", answer = 42, "hello");
        tracing::trace!(target: "mta_testkit::tests", "too quiet");
        let records = capture.records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].level, Level::DEBUG);
        assert_eq!(records[0].target, "mta_testkit::tests");
        assert_eq!(records[0].message(), Some("hello"));
        assert_eq!(
            records[0].fields.get("answer").map(String::as_str),
            Some("42")
        );
    }
}
