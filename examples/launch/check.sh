#!/usr/bin/env bash
#
# Launch-surface snippet gate.
#
# Every .ty under examples/launch/ is a PUBLIC snippet: it appears verbatim in the
# README hero, the website hero, launch posts (HN first comment), and the OG image.
# A broken demo at the moment of peak attention is the single worst self-inflicted
# launch failure, so this gate enforces, at HEAD, that each snippet:
#
#   1. is fmt-clean      (tyra fmt --check)
#   2. type-checks       (tyra check)
#   3. compiles + runs   (tyra run, exit 0)
#   4. prints exactly    (diff against the adjacent <name>.out, when present)
#
# Usage:  bash examples/launch/check.sh [path/to/tyra]   (defaults to ./target/release/tyra)
#
# No silent skips: a .ty with no .out still must compile/run/fmt; the dir must be
# non-empty. Any failure exits non-zero with a diff.

set -euo pipefail

TYRA="${1:-./target/release/tyra}"
DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$DIR/../.." && pwd)"
export TYRA_STDLIB="${TYRA_STDLIB:-$ROOT/stdlib}"

if [ ! -x "$TYRA" ] && ! command -v "$TYRA" >/dev/null 2>&1; then
  echo "launch-gate: tyra binary not found or not executable: $TYRA" >&2
  exit 2
fi

shopt -s nullglob
files=("$DIR"/*.ty)
if [ "${#files[@]}" -eq 0 ]; then
  echo "launch-gate: no .ty snippets found in $DIR" >&2
  exit 1
fi

fail=0
for f in "${files[@]}"; do
  name="$(basename "$f")"
  echo "launch-gate: $name"

  if ! "$TYRA" fmt --check "$f" >/dev/null 2>&1; then
    echo "  FAIL: not fmt-clean — run: tyra fmt $f" >&2
    fail=1; continue
  fi

  if ! "$TYRA" check "$f" >/dev/null 2>&1; then
    echo "  FAIL: tyra check reported errors:" >&2
    "$TYRA" check "$f" 2>&1 | sed 's/^/    /' >&2 || true
    fail=1; continue
  fi

  # Capture stdout byte-for-byte (a temp file preserves trailing newlines that
  # $(...) would strip, so an .out pin catches stray blank lines at EOF too).
  actual="$(mktemp)"
  if ! "$TYRA" run "$f" >"$actual" 2>/dev/null; then
    echo "  FAIL: tyra run exited non-zero" >&2
    "$TYRA" run "$f" 2>&1 | sed 's/^/    /' >&2 || true
    rm -f "$actual"; fail=1; continue
  fi

  expected="${f%.ty}.out"
  if [ -f "$expected" ]; then
    if ! diff -u "$expected" "$actual" >/dev/null 2>&1; then
      echo "  FAIL: stdout differs from $(basename "$expected"):" >&2
      diff -u "$expected" "$actual" | sed 's/^/    /' >&2 || true
      rm -f "$actual"; fail=1; continue
    fi
    echo "  ok (fmt + check + run; output matches $(basename "$expected"))"
  else
    echo "  ok (fmt + check + run; no .out pin)"
  fi
  rm -f "$actual"
done

if [ "$fail" -ne 0 ]; then
  echo "launch-gate: FAILED — fix before launch (these snippets are public)." >&2
  exit 1
fi

echo "launch-gate: all launch snippets are fmt-clean, type-check, compile, and run."
