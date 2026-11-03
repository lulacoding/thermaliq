#!/usr/bin/env bash
# One-time machine setup for ThermalIQ v2 (macOS / Linux).
set -euo pipefail

need() { command -v "$1" >/dev/null 2>&1; }

need cargo || { echo "Install Rust first: https://rustup.rs"; exit 1; }
need node  || echo "warn: node not on PATH — the Ponytail plugin's hooks need it"

# RTK (Rust Token Killer). `rtk gain` only exists in the right package.
if ! rtk gain >/dev/null 2>&1; then
  if need brew; then brew install rtk
  else cargo install --git https://github.com/rtk-ai/rtk
  fi
fi
rtk init -g          # installs the Claude Code hook
rtk init --show

# Reference worktree for the v0 code.
cd "$(git rev-parse --show-toplevel)"
[ -d ../thermaliq-v0 ] || { git fetch origin tag v0-archive --no-tags; git worktree add ../thermaliq-v0 v0-archive; }

echo
echo "Done. Restart Claude Code in this folder."
echo "Ponytail: accept the plugin prompt on first start, or run:"
echo "  /plugin marketplace add DietrichGebert/ponytail"
echo "  /plugin install ponytail@ponytail"
