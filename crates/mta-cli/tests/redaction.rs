//! Property test for the redaction contract (docs/observability.md §5, ADR-0011 decision 8).
//!
//! For a random secret, at every level, with both content opt-ins behaving as if enabled, no
//! captured record may contain the secret. The `Secret` type is the mechanism; this test is the
//! proof that logging a field cannot bypass it.

use mta_domain::Secret;
use mta_testkit::capture::Capture;
use proptest::prelude::*;
use tracing::level_filters::LevelFilter;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    #[test]
    fn a_secret_never_appears_in_a_record(value in "[a-zA-Z0-9]{32}") {
        let secret = Secret::new(format!("sk-{value}"));
        let capture = Capture::start(LevelFilter::TRACE);

        // `tracing` callsites carry a const level, so the five records are spelled out.
        tracing::error!(target: "mta_cli::redaction", secret = ?secret, token = %secret, "error record");
        tracing::warn!(target: "mta_cli::redaction", secret = ?secret, token = %secret, "warn record");
        tracing::info!(target: "mta_cli::redaction", secret = ?secret, token = %secret, "info record");
        tracing::debug!(target: "mta_cli::redaction", secret = ?secret, token = %secret, "debug record");
        tracing::trace!(target: "mta_cli::redaction", secret = ?secret, token = %secret, "trace record");

        let needle = format!("sk-{value}");
        prop_assert!(!capture.contains(&needle), "secret leaked: {needle}");
        prop_assert!(capture.contains("redacted"), "redaction marker missing");
        prop_assert_eq!(capture.records().len(), 5);
    }
}
