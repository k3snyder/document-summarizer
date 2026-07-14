#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SKILL_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

DESTS=(
  "$HOME/.codex/skills/summarizer-cli"
  "$HOME/.claude/skills/summarizer-cli"
)

for dest in "${DESTS[@]}"; do
  mkdir -p "$dest"
  rsync -av --delete \
    --exclude '__pycache__/' \
    --exclude '*.py[cod]' \
    --exclude '.DS_Store' \
    "$SKILL_ROOT/" "$dest/"
done
