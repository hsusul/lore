#!/usr/bin/env bash
# Write a deterministic synthetic Claude/Codex profile for demos and checks.
#
# Usage: scripts/synth-archive.sh <out_dir> <claude_count> <codex_count> <seed>
#
# Prints <out_dir>, then the session counts. Agent homes land in
# <out_dir>/claude/projects and <out_dir>/codex/sessions, so a check can point
# CLAUDE_CONFIG_DIR and CODEX_HOME at them instead of a real agent history.
# The same arguments always produce the same files. Exits with the example's
# status: non-zero with a usage line on a missing or bad argument.
set -euo pipefail
# Keep the caller's cwd so a relative <out_dir> resolves where the caller is.
root="$(cd "$(dirname "$0")/.." && pwd)"

exec cargo run --release -q --manifest-path "$root/Cargo.toml" -p lore-core --example synth -- "$@"
