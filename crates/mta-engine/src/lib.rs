//! Command processor, effects, scheduler, and run executor for `mta`.
//!
//! M0 only establishes the crate boundary. The event-sourced engine lands in M1/M4.

#![forbid(unsafe_code)]
