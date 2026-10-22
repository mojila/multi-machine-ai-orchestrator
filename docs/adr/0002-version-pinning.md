# ADR-0002: Pin MSRV 1.94 and exact pre-1.0 ratatui crates

- **Status:** Proposed
- **Date:** 2026-10-21
- **Deciders:** project owner
- **Related:** [PLAN.md §3.2](../PLAN.md#32-workspace-layout),
  [ADR-0011 diagnostics](0011-observability-and-diagnostics.md)

## Context

The dependency set is young and moves quickly:

- sqlx 0.9 declares `rust-version = 1.94`, which is above ratatui's 1.88 and the rest of the stack.
- ratatui 0.30 is a facade over `ratatui-core 0.1`, `ratatui-widgets 0.3`, and `ratatui-crossterm 0.1`
  — all pre-1.0, so minor bumps can break the API.
- crossterm must match the version linked by `ratatui-crossterm` (0.29); two crossterm versions in one
  binary produce confusing trait/backend errors.
- `rust-toolchain.toml` with `channel = "stable"` would silently pick a toolchain older than the
  crates require on some CI images, failing late and confusingly.
- `sysinfo` crossed an MSRV line inside its own 0.38 → 0.39 upgrade: **0.38.4 needs Rust 1.88, while
  0.39.3 and later need 1.95**. Pinning "latest" would drag the whole workspace's MSRV with it for a
  presentation-only feature.

## Decision

- Pin the workspace to Rust **1.94** via `rust-version` and `rust-toolchain.toml` (`channel = "1.94"`).
- Commit `Cargo.lock` (binary project).
- Pin `ratatui = "=0.30.2"`, `ratatui-textarea = "=0.9.3"`, and crossterm 0.29; enable only the
  features actually used, including `event-stream` for `EventStream`.
- Pin `sysinfo = "=0.38.4"` (MSRV 1.88) rather than 0.39.x (MSRV 1.95). Taking 0.39+ is an explicit
  trigger to bump `rust-version` **and** `rust-toolchain.toml` in one deliberate change, not a
  side effect of a dependency refresh.
- Pin the diagnostics stack exactly: `tracing = "=0.1.44"`, `tracing-subscriber = "=0.3.23"`
  (`default-features = false`; features `std`, `fmt`, `ansi`, `json`, `tracing-log`), and
  `tracing-appender = "=0.2.5"`. Their declared MSRVs (1.65, 1.65, 1.63) were verified against crates.io
  metadata before pinning rather than assumed — the check that would have caught the `sysinfo` trap
  earlier. `env-filter` is not enabled; see [ADR-0011](0011-observability-and-diagnostics.md).
- Fail CI on duplicate terminal crates with `cargo tree -d`.
- Upgrade only via a dedicated PR with snapshots re-baselined and an ADR amendment when a breaking
  change is accepted.

## Consequences

**Positive:** reproducible builds; no surprise MSRV failures; no duplicate-backend class of bugs;
upgrades are reviewable, isolated changes.

**Negative:** manual upgrade cadence; we cannot adopt upstream fixes automatically; exact pins can
conflict with downstream users if this ever becomes a library.

**Follow-ups:** a quarterly (or security-driven) dependency review; `cargo deny` advisory checks; keep
the ratatui surface minimal (no unstable features) so upgrades stay mechanical. Any dependency whose
newest release raises its declared `rust-version` above 1.94 is held at the last compatible version
(`sysinfo` is the first such case) and the MSRV bump is its own reviewed change.
