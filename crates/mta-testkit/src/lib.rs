//! Dev-only test helpers for `mta`. Never shipped (docs/build.md §5).
//!
//! Tests that need to assert on diagnostic records use [`capture::Capture`] instead of installing
//! their own subscriber per test file.

#![forbid(unsafe_code)]

pub mod capture;
