#!/usr/bin/env bash
# Smoke tests for project lifecycle commands:
#   tyra new, zero-arg run/build/check, --release build
#
# Usage: bash tools/scripts/lifecycle_smoke.sh [tyra-binary-path]
#
# Requires a working `tyra` binary and a writable temp directory.
# Exits 0 if all checks pass, 1 on any failure.

set -euo pipefail

_TYRA_ARG="${1:-tyra}"
# Resolve to an absolute path so subshells with `cd` still find the binary.
if [[ -x "$_TYRA_ARG" ]]; then
  TYRA="$(cd "$(dirname "$_TYRA_ARG")" && pwd)/$(basename "$_TYRA_ARG")"
elif command -v "$_TYRA_ARG" &>/dev/null; then
  TYRA="$(command -v "$_TYRA_ARG")"
else
  echo "error: tyra binary not found: '${_TYRA_ARG}'" >&2
  exit 1
fi

TMPDIR_BASE="$(mktemp -d)"
FAIL=0
PASS=0

pass() { echo "ok  - $*"; PASS=$((PASS + 1)); }
fail() { echo "not ok - $*"; FAIL=$((FAIL + 1)); }

cleanup() { rm -rf "$TMPDIR_BASE"; }
trap cleanup EXIT

# ---------------------------------------------------------------------------
# 1. tyra new creates the expected directory layout
# ---------------------------------------------------------------------------
APP_DIR="$TMPDIR_BASE/myapp"
(cd "$TMPDIR_BASE" && "$TYRA" new myapp) 2>/dev/null || true

if [[ -f "$APP_DIR/Tyra.toml" ]]; then
  pass "tyra new creates Tyra.toml"
else
  fail "tyra new creates Tyra.toml (not found at $APP_DIR/Tyra.toml)"
fi

if [[ -f "$APP_DIR/src/myapp.ty" ]]; then
  pass "tyra new creates src/<name>.ty"
else
  fail "tyra new creates src/<name>.ty (not found)"
fi

# ---------------------------------------------------------------------------
# 2. tyra new rejects an existing directory
# ---------------------------------------------------------------------------
NEW_EXIST_OUT="$(cd "$TMPDIR_BASE" && "$TYRA" new myapp 2>&1)" || true
if echo "$NEW_EXIST_OUT" | grep -qiE "error|exist|already"; then
  pass "tyra new rejects existing directory"
else
  fail "tyra new rejects existing directory (expected error output, got: $NEW_EXIST_OUT)"
fi

# ---------------------------------------------------------------------------
# 3. tyra new --lib creates a library project
# ---------------------------------------------------------------------------
LIB_DIR="$TMPDIR_BASE/mylib"
(cd "$TMPDIR_BASE" && "$TYRA" new mylib --lib) 2>/dev/null || true
if [[ -f "$LIB_DIR/src/mylib.ty" ]]; then
  pass "tyra new --lib creates src/<name>.ty"
else
  fail "tyra new --lib creates src/<name>.ty (not found)"
fi

# ---------------------------------------------------------------------------
# 4. zero-arg tyra check inside a project directory
# ---------------------------------------------------------------------------
if (cd "$APP_DIR" && "$TYRA" check) >/dev/null 2>&1; then
  pass "zero-arg tyra check passes inside project dir"
else
  fail "zero-arg tyra check failed inside project dir"
fi

# ---------------------------------------------------------------------------
# 5. zero-arg tyra run inside a project directory
# ---------------------------------------------------------------------------
RUN_OUTPUT="$(cd "$APP_DIR" && "$TYRA" run 2>&1)" || true
if echo "$RUN_OUTPUT" | grep -q "Hello, Tyra"; then
  pass "zero-arg tyra run prints Hello, Tyra!"
else
  fail "zero-arg tyra run output did not contain 'Hello, Tyra!' (got: $RUN_OUTPUT)"
fi

# ---------------------------------------------------------------------------
# 6. zero-arg tyra build inside a project directory
# ---------------------------------------------------------------------------
(cd "$APP_DIR" && "$TYRA" build) >/dev/null 2>&1 || true
if [[ -x "$APP_DIR/myapp" ]]; then
  pass "zero-arg tyra build produces executable at project root"
else
  fail "zero-arg tyra build did not produce $APP_DIR/myapp"
fi

# ---------------------------------------------------------------------------
# 7. tyra build --release produces an executable
# ---------------------------------------------------------------------------
rm -f "$APP_DIR/myapp"
(cd "$APP_DIR" && "$TYRA" build --release) >/dev/null 2>&1 || true
if [[ -x "$APP_DIR/myapp" ]]; then
  pass "tyra build --release produces executable"
else
  fail "tyra build --release did not produce $APP_DIR/myapp"
fi

# ---------------------------------------------------------------------------
# 8. tyra build --release -o <path> writes to the given path
# ---------------------------------------------------------------------------
OUT_PATH="$TMPDIR_BASE/myapp_out"
(cd "$APP_DIR" && "$TYRA" build --release -o "$OUT_PATH") >/dev/null 2>&1 || true
if [[ -x "$OUT_PATH" ]]; then
  pass "tyra build --release -o <path> writes to explicit path"
else
  fail "tyra build --release -o <path> did not write to $OUT_PATH"
fi

# ---------------------------------------------------------------------------
# 9. zero-arg tyra check outside a project directory fails gracefully
# ---------------------------------------------------------------------------
CHECK_OUTSIDE_OUT="$(cd "$TMPDIR_BASE" && "$TYRA" check 2>&1)" || true
if echo "$CHECK_OUTSIDE_OUT" | grep -qiE "error|no.*project|Tyra\.toml"; then
  pass "zero-arg tyra check outside project gives a clear error"
else
  fail "zero-arg tyra check outside project did not give an expected error message (got: $CHECK_OUTSIDE_OUT)"
fi

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
echo ""
echo "passed: $PASS"
echo "failed: $FAIL"
echo ""
if [[ "$FAIL" -gt 0 ]]; then
  exit 1
fi
