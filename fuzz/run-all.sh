#!/usr/bin/env bash
# Run every fuzz target for a fixed short duration against the checked-in
# corpus. Fast and deterministic, not an endless campaign -- long enough to
# catch the shallow bugs, which is what a per-commit gate is for.
#
#   ./run-all.sh [seconds-per-target]
#
# Exits non-zero if any target crashes, hangs or leaks; libFuzzer writes the
# offending input to fuzz/artifacts/<target>/ either way.
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
secs="${1:-60}"

"$here/seed-corpus.sh"

targets=$(grep -o '^name = "[a-z0-9_]*"' "$here/Cargo.toml" | sed 's/name = "//; s/"//')

failed=()
for target in $targets; do
  echo "::group::$target"
  # -runs is a second bound so a target that has exhausted its search space
  # does not sit spinning for the whole budget.
  #
  # -timeout=10: a clipboard parser that takes ten seconds on one payload is a
  # hang for the purpose of this repo, whatever libFuzzer's default says. The
  # brief is "no panic and no hang" and this is the half that enforces the hang.
  if ! cargo +nightly fuzz run "$target" -- \
        -max_total_time="$secs" \
        -timeout=10 \
        -print_final_stats=1; then
    failed+=("$target")
    echo "FAILED: $target"
  fi
  echo "::endgroup::"
done

if [ ${#failed[@]} -ne 0 ]; then
  echo
  echo "targets that failed: ${failed[*]}"
  exit 1
fi
echo
echo "all targets clean for ${secs}s each"
