# ADR-0005: Build artifacts and binary size are budgeted, not unbounded

- **Status:** Accepted
- **Date:** 2026-10-21
- **Deciders:** project owner
- **Related:** [docs/build.md](../build.md), [size-budget.toml](../../size-budget.toml),
  [ADR-0002 version pinning](0002-version-pinning.md)

## Context

Rust build artifacts are append-mostly. For this stack (tokio, reqwest/rustls, sqlx with bundled
SQLite, ratatui, clap, plus test binaries) a default `cargo build` + `cargo test` + `cargo build
--release` produces several gigabytes, dominated by DWARF debug info in dependencies and by the
incremental cache. Extra profiles (`doc`, `nextest`, `llvm-cov-target`), removed dependencies, and
cross-compilation triples each add more. Nothing in Cargo prunes any of it.

The concrete symptoms we want to avoid: a `target/` that doubles every few months, CI caches evicted
because they are oversized, release binaries that creep upward unnoticed, and "reclaim space" being a
manual ritual nobody performs.

Options considered:

1. Do nothing; document `cargo clean` in the README.
2. Keep `target/` inside the repo but prune it with a script + budgets.
3. Move `target/` outside the repo (shared `CARGO_TARGET_DIR`) and rely on a global cache manager.

Option 3 trades one unbounded directory for another and makes worktrees thrash each other's caches;
option 1 reproduces exactly the problem described.

## Decision

`target/` stays inside the project (gitignored) and is **bounded by a committed budget**:

1. Baseline profiles in `Cargo.toml`: `[profile.dev] debug = "line-tables-only"`,
   `[profile.dev.package."*"] debug = false`, `[profile.test] debug = "line-tables-only"`,
   `[profile.release] debug = 0, strip = "debuginfo", lto = "thin", codegen-units = 1`.
2. `size-budget.toml` holds MiB budgets for the target directory and the stripped release binary;
   values start generous and are calibrated in M1 and tightened in M6.
3. `scripts/size-report.sh [--check]` reports sizes and enforces the budget; CI runs `--check` on
   merge to `main`. `scripts/prune-target.sh [--aggressive]` reclaims space safely.
4. CI sets `CARGO_INCREMENTAL=0`, caches keyed on `Cargo.lock`, and gives coverage (`target/cov`) and
   distribution (`target/dist`) their own target dirs so the dev target is never inflated by them.
5. Raising a budget requires a reviewed commit with the measured size, the top consumers, and the
   mitigation attempted.
6. `panic = "abort"` is rejected for v0.1: it shrinks the binary but breaks the test harness and the
   panic-hook/terminal-restore design.

## Consequences

**Positive:** disk usage stays predictable on a laptop and in CI; growth becomes visible in diffs; the
release binary has a number attached to it; cleanup is a one-liner instead of a judgement call.

**Negative:** slightly worse debugging into dependencies (`debug = false` there) and slower release
builds from `lto`/`codegen-units = 1`; two more files (`size-budget.toml`, scripts) to maintain; CI
must be able to fail on size, which some projects find noisy.

**Follow-ups:** calibrate budgets in M1 (first build) and M6 (release binary); optionally record a
`docs/reports/build-size.tsv` trend from the CI size job; revisit feature trimming whenever a dependency
gets `default-features` enabled by accident.
