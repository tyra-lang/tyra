#!/usr/bin/env bash
# Codegen equivalence harness (v0.9.0 plan §検証 G2).
#
# Verifies that two `tyra` binaries produce byte-identical *runtime behavior*
# on the static corpus.  Intended for the inkwell IR migration: build a
# pre-migration `tyra` and a post-migration `tyra`, then confirm every corpus
# program behaves identically (the SSA naming in the emitted .ll changes, so a
# textual golden diff of the IR is useless — this compares observable behavior).
#
# Usage:
#   bash bench/static-corpus/codegen-equivalence.sh <old-tyra> <new-tyra> [--include-network]
#
# File classes (mirrors check.sh so the same corpus is covered consistently):
#   *.tyra (non-test, non-bad)  -> `tyra build -o <tmp>` then run the binary;
#                                  compare combined stdout+stderr and exit code.
#   *_test.tyra                 -> `tyra test`; compare output (per-file `# time:`
#                                  TAP timing lines are stripped — nondeterministic).
#   bad/*.tyra                  -> EXCLUDED: these fail type-checking and never
#                                  reach codegen, so they are irrelevant to codegen
#                                  equivalence (covered by check.sh / G1 verify).
#   win/*.tyra                  -> EXCLUDED: Windows-only, not buildable on the host.
#
# Exits 0 only if every compared program matches.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [[ $# -lt 2 ]]; then
  echo "usage: codegen-equivalence.sh <old-tyra> <new-tyra> [--include-network]" >&2
  exit 2
fi
OLD="$1"
NEW="$2"
INCLUDE_NETWORK=0
for arg in "$@"; do
  [[ "$arg" == "--include-network" ]] && INCLUDE_NETWORK=1
done

# Skip patterns: nondeterministic output cannot be byte-compared across two runs.
#   20-math_bench — emits wall-clock ns timings (differs every run).
#   04 / 08       — require live network access (also skipped by check.sh).
SKIP_PATTERNS=("20-math_bench")
if [[ "$INCLUDE_NETWORK" -eq 0 ]]; then
  SKIP_PATTERNS+=("04-http-handler" "08-async-tasks")
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

FAIL=0
PASS=0
SKIP=0
UNBUILDABLE=()  # tracked (allowlisted) programs both compilers fail to build

# Allowlist of positive-corpus programs that are KNOWN to fail `tyra build` on
# the current tree, with the reason and tracking task. A program here that both
# binaries fail to build is reported but does NOT fail the run; ANY positive
# program NOT listed here that fails to build is a hard FAIL (G2 guarantees
# build+run, so a shared codegen regression must never slip through silently).
# Prune entries as the underlying defects are fixed — the harness flags any
# listed program that starts building again.
KNOWN_UNBUILDABLE=(
)

is_known_unbuildable() {
  local b="$1"
  for k in "${KNOWN_UNBUILDABLE[@]:-}"; do
    [[ "$b" == "$k" ]] && return 0
  done
  return 1
}

# Strip nondeterministic TAP timing comments emitted by `tyra test`.
strip_nondeterministic() {
  sed -e 's/^# time:.*$/# time: <stripped>/'
}

shopt -s nullglob

# ---- Class 1: buildable programs (build + run, compare behavior) ----
for f in "$SCRIPT_DIR"/*.tyra; do
  base="$(basename "$f" .tyra)"
  [[ "$base" == *_test ]] && continue

  skip=0
  for pat in "${SKIP_PATTERNS[@]:-}"; do
    [[ "$base" == *"$pat"* ]] && skip=1 && break
  done
  if [[ "$skip" -eq 1 ]]; then
    echo "SKIP $base (excluded)"
    SKIP=$((SKIP + 1))
    continue
  fi

  obin="$TMP/old_$base"
  nbin="$TMP/new_$base"
  set +e
  "$OLD" build "$f" -o "$obin" > "$TMP/o_build.log" 2>&1
  obuild=$?
  "$NEW" build "$f" -o "$nbin" > "$TMP/n_build.log" 2>&1
  nbuild=$?
  set -e

  if [[ $obuild -ne 0 || $nbuild -ne 0 ]]; then
    if [[ $obuild -ne $nbuild ]]; then
      if [[ $obuild -ne 0 && $nbuild -eq 0 ]] && is_known_unbuildable "$base"; then
        # Allowlisted program where legacy (old) fails but inkwell (new) succeeds:
        # inkwell has fixed a legacy codegen defect. Report as KNOWN-IMPROVED so
        # the harness stays green while still surfacing the outstanding work item.
        # Guard is obuild!=0 && nbuild==0 — the reverse (old=0, new!=0) is a real
        # inkwell regression and must not be silently swallowed here.
        echo "KNOWN-IMPROVED $base (old=$obuild new=$nbuild - legacy regression fixed by inkwell)"
        UNBUILDABLE+=("$base (old=$obuild->new=$nbuild)")
      else
        echo "FAIL $base (build exit differs: old=$obuild new=$nbuild)"
        FAIL=$((FAIL + 1))
      fi
    elif is_known_unbuildable "$base"; then
      # Tracked, deliberately-deferred breakage (codegen defects → Theme A,
      # or the string.concat / check-vs-build gap → task #16). Reported but
      # not failed, so the harness stays green on the current tree while still
      # catching NEW regressions in the else branch below.
      echo "KNOWN-UNBUILDABLE $base (deferred, exit=$obuild)"
      UNBUILDABLE+=("$base (exit=$obuild)")
    else
      # A positive-corpus program that BOTH binaries fail to build and is NOT
      # allowlisted = a real regression. G2 guarantees build+run, so a shared
      # codegen regression must fail here rather than slip through silently.
      echo "FAIL $base (unexpectedly unbuildable, exit=$obuild — not in KNOWN_UNBUILDABLE)"
      FAIL=$((FAIL + 1))
    fi
    continue
  fi

  # A program that builds but is still allowlisted means the underlying defect
  # was fixed — flag it so KNOWN_UNBUILDABLE gets pruned. (Comparison proceeds.)
  if is_known_unbuildable "$base"; then
    echo "NOTE $base now builds — remove it from KNOWN_UNBUILDABLE"
  fi

  set +e
  oout="$("$obin" 2>&1)"
  oexit=$?
  nout="$("$nbin" 2>&1)"
  nexit=$?
  set -e

  if [[ "$oout" == "$nout" && $oexit -eq $nexit ]]; then
    echo "OK   $base (exit=$oexit)"
    PASS=$((PASS + 1))
  else
    echo "FAIL $base (exit old=$oexit new=$nexit)"
    # `|| true`: diff exits 1 on differences, which under `set -euo pipefail`
    # would abort the script before the FAIL tally — absorb it.
    diff <(printf '%s' "$oout") <(printf '%s' "$nout") | sed 's/^/     /' | head -40 || true
    FAIL=$((FAIL + 1))
  fi
done

# ---- Class 2: test files (tyra test, compare normalized output) ----
for f in "$SCRIPT_DIR"/*_test.tyra; do
  base="$(basename "$f" .tyra)"

  set +e
  oraw="$("$OLD" test "$f" 2>&1)"
  oexit=$?
  nraw="$("$NEW" test "$f" 2>&1)"
  nexit=$?
  set -e

  oout="$(printf '%s' "$oraw" | strip_nondeterministic)"
  nout="$(printf '%s' "$nraw" | strip_nondeterministic)"

  if [[ "$oout" == "$nout" && $oexit -eq $nexit ]]; then
    echo "OK   $base (tyra test, exit=$oexit)"
    PASS=$((PASS + 1))
  else
    echo "FAIL $base (tyra test, exit old=$oexit new=$nexit)"
    # `|| true`: diff exits 1 on differences, which under `set -euo pipefail`
    # would abort the script before the FAIL tally — absorb it.
    diff <(printf '%s' "$oout") <(printf '%s' "$nout") | sed 's/^/     /' | head -40 || true
    FAIL=$((FAIL + 1))
  fi
done

echo ""
echo "Results: $PASS passed, $FAIL failed, $SKIP skipped, ${#UNBUILDABLE[@]} known-unbuildable"
if [[ "${#UNBUILDABLE[@]}" -gt 0 ]]; then
  echo ""
  echo "KNOWN-UNBUILDABLE (allowlisted: tyra check passes but tyra build fails on BOTH"
  echo "binaries; tracked for fix — see KNOWN_UNBUILDABLE in this script):"
  for u in "${UNBUILDABLE[@]}"; do
    echo "  - $u"
  done
fi
# Exit nonzero on any mismatch OR any NEW (non-allowlisted) unbuildable program.
[[ "$FAIL" -eq 0 ]]
