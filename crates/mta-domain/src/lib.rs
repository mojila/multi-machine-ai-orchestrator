//! Pure domain types for `mta`.
//!
//! This crate is the deterministic core. It must not depend on `tracing`, `sqlx`, `reqwest`,
//! `tokio`, or any other I/O crate; CI asserts that with `cargo tree -p mta-domain`.
//! Deciders that live here are pure, so they cannot log (ADR-0011 decision 7).

pub mod secret;

pub use secret::Secret;
