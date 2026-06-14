#!/usr/bin/env bash
# Interpret equivalence harness (Playground plan Phase 3).
#
# Verifies that `tyra run --interpret` produces byte-identical observable
# behavior (stdout + stderr + exit code) compared with native `tyra run` on
# the static corpus.
#
# This is the critical parity gate before the WASM Playground is published.
# All differences are interpreter semantic bugs and must be root-cause fixed —
# the test criteria must not be loosened.
#
# Usage:
#   bash bench/static-corpus/interpret-equivalence.sh <tyra-binary>
#
# File classes:
#   *.ty (non-test, non-bad)  -> `tyra run` vs `tyra run --interpret`; compare
#                                  stdout, stderr, and exit code separately.
#   *_test.ty                 -> EXCLUDED: no fn main; cannot be run directly.
#   bad/*.ty                  -> EXCLUDED: fail check; never reach the interpreter.
#   win/*.ty                  -> EXCLUDED: Windows-only, not runnable on this host.
#
# Skip patterns (unsupported in interpreter — explicitly logged):
#   04-http-handler          — uses io/http (unsupported in interpreter)
#   06-cli-args              — uses sys.args() (nondeterministic across invocations)
#   08-async-tasks           — uses core.tasks async (unsupported in interpreter)
#   09-error-handling        — uses fs (unsupported in interpreter)
#   20-math_bench            — emits wall-clock ns timings (nondeterministic output)
#   25-nested-match-map-get  — uses io (unsupported in interpreter)
#
# These skips are EXPLICITLY reported (not silently dropped) so the harness
# cannot mask coverage gaps (feedback_machine_readable_contracts rule).
#
# Exits 0 only if every compared program matches.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [[ $# -lt 1 ]]; then
  echo "usage: interpret-equivalence.sh <tyra-binary>" >&2
  exit 2
fi
TYRA="$1"

# Returns 0 (true) and prints a skip reason if this file stem should be skipped.
skip_reason() {
  local base="$1"
  case "$base" in
    04-http-handler)           echo "uses io/http (unsupported in interpreter)"; return 0 ;;
    06-cli-args)               echo "uses sys.args() (nondeterministic across invocations)"; return 0 ;;
    08-async-tasks)            echo "uses core.tasks async (unsupported in interpreter)"; return 0 ;;
    09-error-handling)         echo "uses fs (unsupported in interpreter)"; return 0 ;;
    20-math_bench)             echo "emits wall-clock timings (nondeterministic output)"; return 0 ;;
    25-nested-match-map-get)   echo "uses io (unsupported in interpreter)"; return 0 ;;
  esac
  return 1
}

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

FAIL=0
PASS=0
SKIP=0

shopt -s nullglob

for f in "$SCRIPT_DIR"/*.ty; do
  base="$(basename "$f" .ty)"
  [[ "$base" == *_test ]] && continue
  # Exclude synthesized temp runner files (e.g. __tyra_test_runner_*)
  [[ "$base" == __* ]] && continue

  reason=""
  if reason="$(skip_reason "$base")"; then
    echo "SKIP $base ($reason)"
    SKIP=$((SKIP + 1))
    continue
  fi

  # --- Native run ---
  native_out="$TMP/${base}_native.stdout"
  native_err="$TMP/${base}_native.stderr"
  set +e
  "$TYRA" run "$f" > "$native_out" 2> "$native_err"
  native_exit=$?
  set -e

  if [[ $native_exit -ne 0 ]]; then
    # Native run failing means the corpus file itself has an issue unrelated to
    # the interpreter. Report but do not count as an interpreter parity failure.
    echo "SKIP $base (native run failed with exit=$native_exit — corpus issue, not interpreter)"
    SKIP=$((SKIP + 1))
    continue
  fi

  # --- Interpreter run ---
  interp_out="$TMP/${base}_interp.stdout"
  interp_err="$TMP/${base}_interp.stderr"
  set +e
  "$TYRA" run --interpret "$f" > "$interp_out" 2> "$interp_err"
  interp_exit=$?
  set -e

  # Compare stdout, stderr, exit code.
  stdout_match=1
  stderr_match=1
  exit_match=1
  diff <(cat "$native_out") <(cat "$interp_out") > "$TMP/${base}.stdout.diff" 2>&1 || stdout_match=0
  diff <(cat "$native_err") <(cat "$interp_err") > "$TMP/${base}.stderr.diff" 2>&1 || stderr_match=0
  [[ $native_exit -eq $interp_exit ]] || exit_match=0

  if [[ $stdout_match -eq 1 && $stderr_match -eq 1 && $exit_match -eq 1 ]]; then
    echo "OK   $base (exit=$native_exit)"
    PASS=$((PASS + 1))
  else
    echo "FAIL $base (native exit=$native_exit, interp exit=$interp_exit)"
    if [[ $stdout_match -eq 0 ]]; then
      echo "     --- stdout diff (native vs interpret) ---"
      head -20 "$TMP/${base}.stdout.diff" | sed 's/^/     /' || true
    fi
    if [[ $stderr_match -eq 0 ]]; then
      echo "     --- stderr diff (native vs interpret) ---"
      head -20 "$TMP/${base}.stderr.diff" | sed 's/^/     /' || true
    fi
    FAIL=$((FAIL + 1))
  fi
done

echo ""
echo "Results: $PASS passed, $FAIL failed, $SKIP skipped"
echo "Skipped $SKIP programs (see reasons above — all skips are explicit, none silent)."
[[ "$FAIL" -eq 0 ]]
