#!/usr/bin/env bash
# Record the README/hero terminal cast: asciinema -> agg (GIF).
# Requires: brew install asciinema agg
#
# Usage: bash scripts/demo/record.sh

set -euo pipefail

DIR="$(cd "$(dirname "$0")" && pwd)"
ASSETS="$DIR/../../docs/assets"
mkdir -p "$ASSETS"

CAST="$ASSETS/demo.cast"
GIF="$ASSETS/demo.gif"

rm -f "$CAST"
asciinema rec --overwrite --idle-time-limit 1.5 \
  -c "bash '$DIR/script.sh'" \
  "$CAST"

agg --theme monokai --font-size 16 --cols 100 --rows 24 "$CAST" "$GIF"

echo "Recorded: $CAST"
echo "Rendered: $GIF ($(du -h "$GIF" | cut -f1))"
echo "Play locally: asciinema play $CAST"
