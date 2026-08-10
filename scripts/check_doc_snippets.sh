#!/usr/bin/env bash
#
# README code-snippet gate.
#
# Every ```tyra fenced block in README.md / README.ja.md is a public-facing
# example (Hello World, taste of the type system, quick starts, ...). A
# broken snippet there is a first-impression failure, so this gate enforces
# that each ```tyra block, extracted as a standalone file, is fmt-clean and
# type-checks.
#
# Blocks tagged ```tyra-fragment (illustrative fragments — partial code,
# `# ...` elisions) are intentionally skipped: they are not meant to compile
# standalone.
#
# Usage: bash scripts/check_doc_snippets.sh [path/to/tyra]   (defaults to ./target/debug/tyra)

set -euo pipefail

TYRA="${1:-./target/debug/tyra}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
export TYRA_STDLIB="${TYRA_STDLIB:-$ROOT/stdlib}"

if [ ! -x "$TYRA" ] && ! command -v "$TYRA" >/dev/null 2>&1; then
  echo "check_doc_snippets: tyra binary not found or not executable: $TYRA" >&2
  exit 2
fi

# Emits every ```tyra ... ``` block as <outdir>/<basename>-snippet-NN.ty.
# ```tyra-fragment and any other info string are skipped: the fence line
# must read exactly "```tyra".
extract_tyra_blocks() {
  local src="$1" outdir="$2" base="$3"
  awk -v outdir="$outdir" -v base="$base" '
    /^```tyra\r?$/ && !in_block { n++; in_block=1; fname = outdir "/" base "-snippet-" sprintf("%02d", n) ".ty"; next }
    /^```\r?$/ { if (in_block) { in_block=0; close(fname) }; next }
    { if (in_block) { sub(/\r$/, ""); print > fname } }
  ' "$src"
}

WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT

PASS=0
FAIL=0

shopt -s nullglob

for readme in README.md README.ja.md; do
  src="$ROOT/$readme"
  base="${readme%.md}"
  extract_tyra_blocks "$src" "$WORKDIR" "$base"

  readme_files=("$WORKDIR/$base"-snippet-*.ty)
  if [ "${#readme_files[@]}" -eq 0 ]; then
    echo "check_doc_snippets: no \`\`\`tyra blocks found in $readme" >&2
    exit 1
  fi
done

files=("$WORKDIR"/*.ty)

for f in "${files[@]}"; do
  name="$(basename "$f")"

  if ! fmt_out="$("$TYRA" fmt --check "$f" 2>&1)"; then
    echo "FAIL: $name — not fmt-clean" >&2
    echo "$fmt_out" | sed 's/^/  /' >&2
    FAIL=$((FAIL + 1))
    continue
  fi

  if ! check_out="$("$TYRA" check "$f" 2>&1)"; then
    echo "FAIL: $name — tyra check reported errors" >&2
    echo "$check_out" | sed 's/^/  /' >&2
    FAIL=$((FAIL + 1))
    continue
  fi

  echo "PASS: $name"
  PASS=$((PASS + 1))
done

echo "check_doc_snippets: $PASS passed, $FAIL failed"

if [ "$FAIL" -ne 0 ]; then
  exit 1
fi
