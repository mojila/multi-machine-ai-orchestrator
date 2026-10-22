#!/usr/bin/env bash
# Report (and optionally enforce) build-artifact budgets. See docs/build.md.
#
#   scripts/size-report.sh            # human-readable report
#   scripts/size-report.sh --check    # exit 1 when over budget (CI)
#
# Budgets come from size-budget.toml. Target dir is CARGO_TARGET_DIR, else ./target.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUDGET_FILE="$ROOT/size-budget.toml"
TARGET="${CARGO_TARGET_DIR:-$ROOT/target}"
CHECK=0

for arg in "$@"; do
  case "$arg" in
    --check) CHECK=1 ;;
    --target=*) TARGET="${arg#--target=}" ;;
    -h|--help) sed -n '2,7p' "$0"; exit 0 ;;
    *) echo "size-report: unknown argument '$arg'" >&2; exit 2 ;;
  esac
done

budget() {
  # budget <key> -> integer MiB, empty when missing
  [ -f "$BUDGET_FILE" ] || return 0
  awk -F'=' -v k="$1" '
    $1 ~ "^[[:space:]]*" k "[[:space:]]*$" { gsub(/[^0-9]/, "", $2); if ($2 != "") print $2; exit }
  ' "$BUDGET_FILE"
}

mib() { du -sm "$1" 2>/dev/null | cut -f1; }

if [ ! -d "$TARGET" ]; then
  echo "size-report: no target directory at $TARGET (nothing built yet) — ok"
  exit 0
fi

total="$(mib "$TARGET")"
b_total="$(budget total_mb)"

printf 'target directory: %s\n' "$TARGET"
printf 'total:            %s MiB' "$total"
[ -n "$b_total" ] && printf '  (budget %s MiB)' "$b_total"
printf '\n\n'

echo "sections:"
for dir in debug debug/incremental release doc nextest llvm-cov-target cov dist distrib; do
  if [ -d "$TARGET/$dir" ]; then
    printf '  %-18s %6s MiB\n' "$dir" "$(mib "$TARGET/$dir")"
  fi
done

echo
echo "largest entries:"
du -sm "$TARGET"/* 2>/dev/null | sort -rn | head -10 | sed 's/^/  /'

echo
echo "binaries:"
b_bin="$(budget release_mta_mb)"
for bin in "$TARGET"/release/mta "$TARGET"/debug/mta; do
  if [ -f "$bin" ]; then
    size="$(mib "$bin")"
    printf '  %-28s %6s MiB' "${bin#"$TARGET"/}" "$size"
    if [ "$bin" = "$TARGET/release/mta" ] && [ -n "$b_bin" ]; then
      printf '  (budget %s MiB)' "$b_bin"
      if [ "$size" -gt "$b_bin" ]; then printf '  OVER'; fi
    fi
    printf '\n'
  fi
done
[ ! -f "$TARGET/release/mta" ] && echo "  (no release binary yet)"

status=0
over=()
if [ -n "$b_total" ] && [ "$total" -gt "$b_total" ]; then
  over+=("target total ${total} MiB > ${b_total} MiB")
fi
if [ -n "$b_bin" ] && [ -f "$TARGET/release/mta" ]; then
  size="$(mib "$TARGET/release/mta")"
  [ "$size" -gt "$b_bin" ] && over+=("release binary ${size} MiB > ${b_bin} MiB")
fi
# Section budgets are published in size-budget.toml, so they are enforced too.
check_section() {
  local dir="$1" key="$2" budget_mib size_mib
  budget_mib="$(budget "$key")"
  [ -n "$budget_mib" ] || return 0
  [ -d "$TARGET/$dir" ] || return 0
  size_mib="$(mib "$TARGET/$dir")"
  if [ "$size_mib" -gt "$budget_mib" ]; then
    over+=("$dir ${size_mib} MiB > ${budget_mib} MiB")
  fi
  return 0
}
check_section debug dev_mb
check_section release release_mb
check_section debug/incremental incremental_mb

echo
if [ "${#over[@]}" -eq 0 ]; then
  echo "budget: ok"
else
  echo "budget: OVER"
  for o in "${over[@]}"; do echo "  - $o"; done
  echo
  echo "reclaim with: scripts/prune-target.sh [--aggressive]"
  status=1
fi

if [ "$CHECK" = 1 ]; then
  exit "$status"
fi
exit 0
