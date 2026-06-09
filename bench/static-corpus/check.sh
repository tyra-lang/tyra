#!/usr/bin/env bash
# Compile-check every .tyra file in the static corpus.
# Exits 0 only if ALL files compile without error.
#
# Usage: bash bench/static-corpus/check.sh [tyra-binary-path] [flags]
#
# Flags:
#   --include-network  Include network-dependent files (04, 08).
#   --check-only       Skip "tyra test" execution for *_test.tyra files;
#                      only run "tyra check". Use on platforms where LLVM
#                      codegen is unavailable (e.g. Alpine musl / LLVM 19
#                      where static LLVM libs SIGSEGV during test-binary
#                      initialisation). Type-checking still validates
#                      imports, types, and function signatures.
#
# bad/ subdirectory: files named Exxxx-<slug>.tyra are expected to fail
# with error[Exxxx] in stderr. These exercise the negative corpus.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TYRA="${1:-tyra}"
INCLUDE_NETWORK=0
CHECK_ONLY=0

for arg in "$@"; do
  [[ "$arg" == "--include-network" ]] && INCLUDE_NETWORK=1
  [[ "$arg" == "--check-only" ]] && CHECK_ONLY=1
done

SKIP_PATTERNS=()
if [[ "$INCLUDE_NETWORK" -eq 0 ]]; then
  SKIP_PATTERNS=("04-http-handler" "08-async-tasks")
fi

FAIL=0
PASS=0
SKIP=0

# nullglob: empty globs expand to nothing rather than being passed as literals.
# Set before all loops so the script is order-independent.
shopt -s nullglob

for f in "$SCRIPT_DIR"/*.tyra; do
  base="$(basename "$f" .tyra)"
  skip=0
  # SKIP_PATTERNS may be empty; ${arr[@]:-} suppresses set -u errors on empty arrays.
  for pat in "${SKIP_PATTERNS[@]:-}"; do
    [[ "$base" == *"$pat"* ]] && skip=1 && break
  done
  if [[ "$skip" -eq 1 ]]; then
    echo "SKIP $f (network-dependent)"
    SKIP=$((SKIP + 1))
    continue
  fi
  if "$TYRA" check "$f" > /dev/null 2>&1; then
    echo "OK   $f"
    PASS=$((PASS + 1))
  else
    echo "FAIL $f"
    "$TYRA" check "$f" 2>&1 | sed 's/^/     /'
    FAIL=$((FAIL + 1))
  fi
done

# Negative corpus: files in bad/ must fail with the expected error code.
# *_test.tyra files are tested via tyra test (see below), not tyra check.
for f in "$SCRIPT_DIR"/bad/*.tyra; do
  base="$(basename "$f" .tyra)"
  [[ "$base" == *_test ]] && continue
  if [[ "$base" =~ ^(E[0-9]{4})- ]]; then
    code="${BASH_REMATCH[1]}"
  else
    echo "SKIP $f (no Exxxx prefix in filename)"
    SKIP=$((SKIP + 1))
    continue
  fi

  # Single compiler invocation: capture output and exit code together.
  set +e
  out="$("$TYRA" check "$f" 2>&1)"
  tyra_exit=$?
  set -e

  if [[ $tyra_exit -eq 0 ]]; then
    echo "FAIL $f (expected $code error, but compiled successfully)"
    FAIL=$((FAIL + 1))
  elif printf '%s\n' "$out" | grep -q "error\[$code\]"; then
    echo "OK   $f ($code)"
    PASS=$((PASS + 1))
  else
    echo "FAIL $f (expected $code, got different errors)"
    printf '%s\n' "$out" | sed 's/^/     /'
    FAIL=$((FAIL + 1))
  fi
done

# Test runner: run tyra test on *_test.tyra files in the corpus.
# Skipped when --check-only is passed (see header comment).
if [[ "$CHECK_ONLY" -eq 1 ]]; then
  for f in "$SCRIPT_DIR"/*_test.tyra; do
    echo "SKIP $f (--check-only: tyra test execution skipped on this platform)"
    SKIP=$((SKIP + 1))
  done
else
  for f in "$SCRIPT_DIR"/*_test.tyra; do
    set +e
    out="$("$TYRA" test "$f" 2>&1)"
    tyra_exit=$?
    set -e
    if [[ $tyra_exit -eq 0 ]]; then
      echo "OK   $f (tyra test)"
      PASS=$((PASS + 1))
    else
      echo "FAIL $f (tyra test failed)"
      printf '%s\n' "$out" | sed 's/^/     /'
      FAIL=$((FAIL + 1))
    fi
  done
fi

# Negative test runner: bad/*_test.tyra files must fail tyra test with
# the expected error code. Skipped under --check-only for the same reason.
if [[ "$CHECK_ONLY" -eq 1 ]]; then
  for f in "$SCRIPT_DIR"/bad/*_test.tyra; do
    echo "SKIP $f (--check-only: tyra test execution skipped on this platform)"
    SKIP=$((SKIP + 1))
  done
else
  for f in "$SCRIPT_DIR"/bad/*_test.tyra; do
    base="$(basename "$f" .tyra)"
    if [[ "$base" =~ ^(E[0-9]{4})- ]]; then
      code="${BASH_REMATCH[1]}"
    else
      echo "SKIP $f (no Exxxx prefix in filename)"
      SKIP=$((SKIP + 1))
      continue
    fi

    set +e
    out="$("$TYRA" test "$f" 2>&1)"
    tyra_exit=$?
    set -e

    if [[ $tyra_exit -eq 0 ]]; then
      echo "FAIL $f (expected $code error from tyra test, but it succeeded)"
      FAIL=$((FAIL + 1))
    elif printf '%s\n' "$out" | grep -q "error\[$code\]"; then
      echo "OK   $f ($code)"
      PASS=$((PASS + 1))
    else
      echo "FAIL $f (expected $code, got different output)"
      printf '%s\n' "$out" | sed 's/^/     /'
      FAIL=$((FAIL + 1))
    fi
  done
fi

echo ""
echo "Results: $PASS passed, $FAIL failed, $SKIP skipped"
[[ "$FAIL" -eq 0 ]]
