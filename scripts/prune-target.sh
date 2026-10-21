#!/usr/bin/env bash
# Cheap, safe reclaim of build-artifact space. See docs/build.md.
#
#   scripts/prune-target.sh                 # docs, incremental, caches, stale artifacts
#   scripts/prune-target.sh --aggressive    # + `cargo clean --profile dev|test` (deps re-link)
#
# Never touches source, never deletes Cargo.lock, never runs while a build is in progress.
# `target/insta` (pending snapshot review) is left alone — review with `cargo insta` first.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="${CARGO_TARGET_DIR:-$ROOT/target}"
AGGRESSIVE=0

for arg in "$@"; do
  case "$arg" in
    --aggressive) AGGRESSIVE=1 ;;
    -h|--help) sed -n '2,8p' "$0"; exit 0 ;;
    *) echo "prune-target: unknown argument '$arg'" >&2; exit 2 ;;
  esac
done

[ -d "$TARGET" ] || { echo "prune-target: no target directory at $TARGET — nothing to do"; exit 0; }

before="$(du -sm "$TARGET" | cut -f1)"

# Always disposable: generated docs, incremental caches, tool output dirs.
for dir in doc debug/incremental nextest llvm-cov-target cov dist distrib package doc-tests; do
  rm -rf "${TARGET:?}/$dir"
done

# Stale artifacts from dependencies we no longer build (optional tool).
if command -v cargo-sweep >/dev/null 2>&1 && [ -f "$ROOT/Cargo.toml" ]; then
  (cd "$ROOT" && cargo sweep --time 7d "$TARGET") || true
else
  echo "prune-target: cargo-sweep not installed (optional: cargo install cargo-sweep)"
fi

# Rebuildable from scratch, keeps dependency artifacts: only on request.
if [ "$AGGRESSIVE" = 1 ] && [ -f "$ROOT/Cargo.toml" ]; then
  (cd "$ROOT" && cargo clean --profile dev && cargo clean --profile test) || true
fi

after="$(du -sm "$TARGET" | cut -f1)"
echo "prune-target: ${before} MiB -> ${after} MiB (reclaimed $((before - after)) MiB)"
