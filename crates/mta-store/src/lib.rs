//! Event log, projections, and SQLite persistence for `mta`.
//!
//! M0 only establishes the crate boundary. The event store lands in M1 (ADR-0009).

#![forbid(unsafe_code)]
