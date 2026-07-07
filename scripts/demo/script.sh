#!/usr/bin/env bash
# Scripted demo for the README/hero terminal cast: install -> write -> compile -> run,
# in under 60 seconds. Deterministic and replayable — no manual typing during recording.
# Run via scripts/demo/record.sh (which wraps this in `asciinema rec`), not directly.

set -euo pipefail

DEMO_DIR="$(cd "$(dirname "$0")" && pwd)"
SHOWCASE="$DEMO_DIR/../../examples/launch/showcase.ty"

# Fresh $HOME so the install step behaves exactly like a first-time user's
# machine (no partial ~/.local from previous runs) without touching the real
# environment this script happens to run in. A short fixed path (rather than
# mktemp's long one) keeps the recorded transcript readable.
export HOME="/tmp/tyra-cast-home"
rm -rf "$HOME"
mkdir -p "$HOME"
cd "$HOME"

type_line() {
  local text="$1" i
  for ((i = 0; i < ${#text}; i++)); do
    printf '%s' "${text:$i:1}"
    sleep 0.02
  done
  printf '\n'
}

prompt() {
  printf '\033[1;32m$\033[0m '
  type_line "$1"
}

prompt 'curl -fsSL https://raw.githubusercontent.com/tyra-lang/tyra/main/scripts/install.sh | sh'
# TYRA_INSTALL_SH lets this be pointed at the local, not-yet-pushed copy of
# install.sh while developing/testing the cast; the line above always shows
# the real public one-liner regardless. Unset (default) = fetch the real URL,
# so a recording made with the default is only accurate once install.sh ships.
if [ -n "${TYRA_INSTALL_SH:-}" ]; then
  sh "$TYRA_INSTALL_SH"
else
  curl -fsSL https://raw.githubusercontent.com/tyra-lang/tyra/main/scripts/install.sh | sh
fi
export PATH="${HOME}/.local/bin:${PATH}"
sleep 0.6

cp "$SHOWCASE" pricing.ty

prompt 'cat pricing.ty'
cat pricing.ty
sleep 0.6

prompt 'tyra build pricing.ty -o pricing && ./pricing'
tyra build pricing.ty -o pricing && ./pricing
sleep 1.5
