#!/usr/bin/env bash
# Compile-check every .tyra file in the static corpus.
# Exits 0 only if ALL files compile without error.
#
# Usage: bash bench/static-corpus/check.sh [tyra-binary-path]
#
# Files that require live network access (04, 08) are skipped by default;
# pass --include-network to include them.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TYRA="${1:-tyra}"
INCLUDE_NETWORK=0

for arg in "$@"; do
  [[ "$arg" == "--include-network" ]] && INCLUDE_NETWORK=1
done

SKIP_PATTERNS=()
if [[ "$INCLUDE_NETWORK" -eq 0 ]]; then
  SKIP_PATTERNS=("04-http-handler" "08-async-tasks")
fi

FAIL=0
PASS=0
SKIP=0

for f in "$SCRIPT_DIR"/*.tyra; do
  base="$(basename "$f" .tyra)"
  skip=0
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

echo ""
echo "Results: $PASS passed, $FAIL failed, $SKIP skipped"
[[ "$FAIL" -eq 0 ]]
