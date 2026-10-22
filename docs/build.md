# Build hygiene: bounded `target/` and binary size

Rust build artifacts are append-mostly: every profile, every test binary, every dependency version and
every cross-compilation triple adds to `target/`, and nothing prunes it automatically. Left alone, a
workspace of this size reaches tens of gigabytes. This document defines the guardrails so `target/` and
the release binary stay within a **budget** instead of growing month over month.

**Status:** policy defined now, numbers calibrated in M1 (after the first real build). All budgets live
in [`size-budget.toml`](../size-budget.toml) and are checked by
[`scripts/size-report.sh`](../scripts/size-report.sh).

## 1. Where the growth actually comes from

| Source | Typical size | Why it accumulates | Control |
|---|---|---|---|
| Dependency debug info (DWARF) | **GBs** | every crate builds with `debug = 2`, but we never need locals/types inside `tokio`/`sqlx` | `[profile.dev.package."*"] debug = false` |
| Our crates' debug info | hundreds of MB | full DWARF for our own code | `[profile.dev] debug = "line-tables-only"` (keeps useful backtraces) |
| `target/debug/incremental` | 0.5–3 GB | incremental caches for every changed crate, never collected | `CARGO_INCREMENTAL=0` in CI; periodic `rm -rf target/debug/incremental` locally |
| Test binaries | hundreds of MB | `unittest` + integration + doc tests per crate | line-tables-only debug, single shared test profile |
| Duplicate dependency versions | hundreds of MB | two semver-major versions of the same crate coexist silently | `cargo-deny` `multiple-versions = "deny"` plus the terminal-crate grep in CI ([ADR-0002](adr/0002-version-pinning.md), `deny.toml`) |
| Extra profiles/dirs | 0.2–2 GB each | `release`, `doc`, `target/nextest`, `target/llvm-cov-target`, `target/insta` | `cargo clean --profile …`; coverage/dist get their own target dir; `target/insta` only after reviewing pending snapshots |
| Cross-compilation | 0.5–2 GB per triple | `cargo-dist` linux/macOS/Windows/aarch64 outputs | per-CI-job `CARGO_TARGET_DIR`, never accumulate locally |
| Stale artifacts from removed deps | grows forever | cargo only cleans what it rebuilds | `cargo sweep --time 7d` (optional tool) or `cargo clean` |
| `sccache` / `rust-analyzer` caches | user-level, separate | each has its own store | cap `SCCACHE_CACHE_SIZE`; both are outside `target/` |

## 2. Baseline profiles (committed in `Cargo.toml`)

```toml
# Debug builds stay fast to link but stop hoarding DWARF.
[profile.dev]
debug = "line-tables-only"     # 0 is smaller, but backtraces lose line numbers
incremental = true             # local only; CI sets CARGO_INCREMENTAL=0

# Dependencies never need debug info: this is the single biggest win.
[profile.dev.package."*"]
debug = false

[profile.test]
debug = "line-tables-only"

[profile.release]
debug = 0
strip = "debuginfo"            # keep symbols so panics stay debuggable
lto = "thin"                   # smaller and faster binary; slower build
codegen-units = 1
opt-level = 3

# Cross / distrib builds reuse release but drop the last bytes.
[profile.dist]
inherits = "release"
lto = "fat"
```

Deliberately **not** used in v0.1: `panic = "abort"` — it shrinks the binary but breaks the test harness
and weakens the panic-hook/terminal-restore design ([PLAN.md §3.8](PLAN.md#38-tui-architecture)).

## 3. Day-to-day commands

```sh
scripts/size-report.sh              # report: total, sections, top consumers, binary sizes, budget
scripts/size-report.sh --check      # exit 1 when over budget (what CI runs)

scripts/prune-target.sh             # safe reclaim: docs, incremental, caches, stale artifacts > 7d
scripts/prune-target.sh --aggressive  # also `cargo clean --profile dev|test` (deps stay cached)

CARGO_TARGET_DIR=target/cov cargo llvm-cov   # coverage never touches the dev target
CARGO_TARGET_DIR=target/dist cargo dist      # packaging likewise
```

Underneath, the manual commands (all of these are safe on their own):

```sh
cargo clean --doc                                   # docs are pure cache
rm -rf target/debug/incremental                     # always safe, recreated on next build
cargo clean --profile dev                           # our crates, keeps deps cached
cargo clean --profile test
cargo clean --release
cargo clean -p mta-engine                           # after a refactor, drop one crate
cargo sweep --time 7d target                        # optional: cargo install cargo-sweep
du -sm target/* | sort -rn | head                   # find the current hog
```

Rules of thumb:

- `target/doc`, `target/debug/incremental`, `target/nextest`, `target/llvm-cov-target`, `target/cov`,
  `target/dist` and `target/distrib` are always disposable. `target/insta` holds **pending snapshot
  reviews** — run `cargo insta` before deleting it.
- Do not run `prune-target.sh` while a build is in progress.
- Coverage and packaging run with a **separate** `CARGO_TARGET_DIR` so the dev target is never inflated.
- Never `cargo vendor` into the repo, never commit `target/`, never add a checked-in
  `.cargo/config.toml` that pins a global `target-dir` (it makes worktrees thrash each other's caches).

## 4. CI

- `CARGO_INCREMENTAL=0` and `CARGO_PROFILE_DEV_DEBUG=0` on all jobs.
- Cache `~/.cargo` and use `Swatinem/rust-cache` with `workspace = ". -> target"`; do not cache more
  than the lockfile-keyed target (cache keys include `Cargo.lock` so stale artifacts age out).
- One job runs `scripts/size-report.sh --check` on merge to `main`; a breach fails the job with the
  top consumers printed. Growth trend is optional (see §6).
- `.github/workflows/ci.yml` has four jobs: `fmt` (rustfmt plus a TOML parse of the committed
  manifests), `test` on `ubuntu-latest` and `macos-latest` (clippy `-D warnings` + tests, snapshots
  never auto-updated), `policy` (deny, audit, duplicate terminal crates, `mta-domain` purity) and `size`
  (release build + `size-report.sh --check`).
- Release/dist jobs set their own `CARGO_TARGET_DIR` per triple and upload only the binary; runner
  storage is ephemeral anyway.
- Duplicate crate versions stay a hard gate: `cargo-deny` runs with `multiple-versions = "deny"` and
  every skip in `deny.toml` carries a comment naming the crate that forces the duplicate and when to
  re-check. On top of that, a separate grep over `cargo tree -d` fails on a duplicated **terminal**
  crate (`crossterm`, `ratatui*`) — two terminal stacks in one process is a correctness bug, not only a
  size one.
- The policy job also runs `cargo-audit`, the duplicate-terminal gate, and `cargo tree -p mta-domain`
  checked against a forbidden-crate list (`tokio`, `tracing`, `sqlx`, `reqwest`, …), so the purity rule
  in AGENTS.md §4 is enforced by CI and not only by review. `cargo-audit` is installed from a **pinned
  prebuilt release** (`cargo-audit@0.22.2`): the tool's own dependency tree can require a newer rustc
  than the project's MSRV, and that must never force a toolchain bump. Bump the pin deliberately and
  verify the advisory database still covers the lockfile.

## 5. Binary size budget

`--release` with the profiles above, stripped, should land in the low tens of MiB for this stack
(tokio + reqwest/rustls + sqlx/sqlite + ratatui + clap). The committed budget in `size-budget.toml`
was calibrated against the first real build in M0 (2026-10-22):

| Artifact | Measured | Budget |
|---|---|---|
| `target/` after dev + all-targets + release | 932 MiB | 3072 MiB |
| `target/debug` | 710 MiB | 1600 MiB |
| `target/debug/incremental` | 248 MiB | 800 MiB |
| `target/release` | 223 MiB | 600 MiB |
| `target/release/mta` (2,420,376 bytes) | 2.3 MiB | 20 MiB |

The M0 binary is the floor, not the target: it carries clap, `tracing` + the file sink, `directories`,
`serde`, `sha2`, and ratatui/crossterm, but none of the M1–M6 dependencies (tokio, `reqwest`/rustls,
`sqlx`/sqlite, `toml_edit`). The 20 MiB ceiling is the "low tens of MiB" claim in
[PLAN.md §3.11](PLAN.md#311-bounded-build-artifacts-and-binary-size) made enforceable; M6 re-tightens it
with a measured binary.
`scripts/size-report.sh --check` enforces the total, the three section budgets, and the binary.

Rules:

- Budget is enforced for the shipped `mta` binary only (test binaries are dev-only).
- `strip = "debuginfo"`, not `"symbols"`: panic messages stay readable, budget accounts for it.
- Feature discipline is the other half of binary size: trimmed `reqwest` features, no `tokio` `full`,
  minimal `clap` features, and `tracing` with the feature set chosen rather than defaulted:
  `tracing-subscriber` is taken with `default-features = false` and only
  `std`, `fmt`, `ansi`, `json`, `tracing-log`. **`env-filter` is deliberately excluded** — it pulls
  `regex-automata` + `matchers` (the largest single component of a typical `tracing` stack) to parse a
  grammar that is `target=level` pairs, which `mta` parses itself
  ([observability.md](observability.md) §3). `[workspace.dependencies]` keeps one version per crate.
- **Estimation adds no dependency.** The effort heuristic and the EWMA calibration are plain
  integer/float arithmetic in `mta-domain` ([ADR-0012](adr/0012-task-estimation.md)); there is no
  statistics or ML crate to link, and the release budget is unaffected.
- **Config editing adds one small crate.** `toml_edit = "=0.25.17+spec-1.1.0"` (MIT OR Apache-2.0,
  `rust-version = 1.85`, checked against crates.io) is used by the engine's `ConfigEditor` to preserve a
  user's comments and ordering when the model picker writes a binding or appends a `[[models]]` profile
  ([ADR-0013](adr/0013-model-selection-and-search.md)). It is a parser/writer, not an I/O stack, and sits
  comfortably under the 1.94 MSRV. The model-query parser is hand-written (no regex crate). The release
  budget is expected to absorb it; M6 measures the real binary and tightens `size-budget.toml` only with
  a recorded measurement.
- `mta-testkit` (log capture for tests) is a dev-only member and is **outside** the budget: the budget
  covers the shipped `mta` binary, and a capture helper that never links into it cannot affect it.
- `cargo bloat --release --crates` is the diagnostic when the budget is breached; record the finding in
  an ADR or the release notes rather than silently raising the budget.

## 6. Guardrails and escalation

1. `scripts/size-report.sh --check` fails CI when the `target/` total, a published section (dev,
   release, debug/incremental), or the release binary exceeds `size-budget.toml`. Budget changes are a
   reviewed commit with a one-line reason.
2. Raising a budget is allowed only with: the measured size, the top three consumers, and a mitigation
   attempted (profile change, feature trim, clean-up command). No silent raises.
3. Optional trend: append `date,total_mb,release_mb` to `docs/reports/build-size.tsv` from the CI size
   job, so growth is visible in diffs over time.
4. If `target/` exceeds budget twice in a row, the fix is a build change (profiles, features, splitting
   the workspace's heaviest crate), not another clean-up note.

## 7. Verified facts behind these settings

- `debug = "line-tables-only"` is a valid Cargo profile value; `[profile.dev.package."*"] debug = false`
  applies the override to all dependency packages.
- `strip = "debuginfo"` keeps the symbol table; `strip = "symbols"` would remove it.
- The diagnostics stack was checked against crates.io metadata, not assumed: `tracing` 0.1.44 (MSRV
  1.65), `tracing-subscriber` 0.3.23 (MSRV 1.65), and `tracing-appender` 0.2.5 (MSRV 1.63) all sit
  comfortably under the workspace MSRV, so enabling logging implies no toolchain bump. In
  `tracing-subscriber` 0.3.23 `reload` is not a feature flag (only `std` is needed for it), and
  `RollingFileAppender::builder()` exposes `max_log_files`, which is what `[logging].retain_files` maps
  to.
- `cargo clean` accepts `--profile <name>`, `--doc`, `--release`, `-p <pkg>`, `--target <triple>`.
- `incremental` artifacts live in `target/debug/incremental` and are safe to delete at any time.
- `cargo-sweep` is a third-party tool; pin its version if adopted, and treat it as a convenience, not a
  dependency of the build.
