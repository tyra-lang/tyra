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

# showcase.ty is duplicated verbatim across the launch surface (README hero,
# website hero/playground default, and both launch blog posts in each of the
# two repos). A hand-edit to one copy silently drifts from the CI-gated
# original, so diff every copy against the canonical file here.
canonical="$DIR/showcase.ty"

extract_fenced_tyra() {
  # First ```tyra ... ``` block in a markdown file.
  awk '
    /^```tyra$/ { if (!found) { found=1; flag=1; next } }
    flag && /^```$/ { flag=0 }
    flag
  ' "$1"
}

extract_playground_showcase() {
  # The `showcase` entry in playground.astro's SAMPLES template-literal map:
  # a JS backtick string spanning real newlines, closed by a line that is
  # exactly 'end`,'.
  awk '
    /showcase: `/ { sub(/^.*showcase: `/, ""); print; flag=1; next }
    flag && /^end`,$/ { print "end"; flag=0; next }
    flag
  ' "$1"
}

# Comment-only lines (`#...`) are excluded from the comparison: README.ja.md
# carries Japanese translations of the two header comment lines, which is
# intentional localization, not drift. Code lines must still match exactly.
strip_comments() {
  grep -v '^[[:space:]]*#' "$1" || true
}

check_dup() {
  local label="$1" actual_text="$2"
  local actual_file expected_file
  actual_file="$(mktemp)"
  expected_file="$(mktemp)"
  printf '%s\n' "$actual_text" >"$actual_file"
  cp "$canonical" "$expected_file"
  strip_comments "$actual_file" >"$actual_file.stripped"
  strip_comments "$expected_file" >"$expected_file.stripped"
  mv "$actual_file.stripped" "$actual_file"
  mv "$expected_file.stripped" "$expected_file"
  if ! diff -u "$expected_file" "$actual_file" >/dev/null 2>&1; then
    echo "launch-gate: $label" >&2
    echo "  FAIL: drifted from $(basename "$canonical"):" >&2
    diff -u "$expected_file" "$actual_file" | sed 's/^/    /' >&2 || true
    rm -f "$actual_file" "$expected_file"
    fail=1
    return
  fi
  rm -f "$actual_file" "$expected_file"
  echo "launch-gate: $label"
  echo "  ok (matches $(basename "$canonical"))"
}

# In-repo copies: always present in this repo's CI checkout, so a missing or
# drifted copy is a hard failure.
check_dup "README.md"                "$(extract_fenced_tyra "$ROOT/README.md")"
check_dup "README.ja.md"              "$(extract_fenced_tyra "$ROOT/README.ja.md")"
check_dup "docs/blog/launch-essay.md" "$(extract_fenced_tyra "$ROOT/docs/blog/launch-essay.md")"
check_dup "docs/blog/zenn-intro-ja.md" "$(extract_fenced_tyra "$ROOT/docs/blog/zenn-intro-ja.md")"

# Cross-repo copies: website/ is a separate git repository (tyra-lang/tyra-lang.github.io),
# not part of this repo's CI checkout, so it's only present when both repos are
# checked out side by side (the maintainer's local umbrella dev layout). Best-effort:
# check when present, skip with a note (not a failure) when absent.
WEBSITE="$ROOT/../website"
if [ -d "$WEBSITE" ]; then
  check_dup "website/src/pages/playground.astro"      "$(extract_playground_showcase "$WEBSITE/src/pages/playground.astro")"
  check_dup "website/src/pages/blog/launch-essay.md"   "$(extract_fenced_tyra "$WEBSITE/src/pages/blog/launch-essay.md")"
  check_dup "website/src/pages/blog/zenn-intro-ja.md"  "$(extract_fenced_tyra "$WEBSITE/src/pages/blog/zenn-intro-ja.md")"
else
  echo "launch-gate: website/ not found alongside this checkout — skipping cross-repo dup checks (expected in tyra's own CI)"
fi

if [ "$fail" -ne 0 ]; then
  echo "launch-gate: FAILED — fix before launch (these snippets are public)." >&2
  exit 1
fi

echo "launch-gate: all launch snippets are fmt-clean, type-check, compile, and run."
