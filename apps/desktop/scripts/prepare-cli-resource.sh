#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
REPO_ROOT="$(cd "$APP_DIR/../.." && pwd)"

cargo +1.88.0 build --release -p summarizer-cli --manifest-path "$REPO_ROOT/backend-rs/Cargo.toml"
mkdir -p "$APP_DIR/src-tauri/resources/bin"
cp "$REPO_ROOT/backend-rs/target/release/summarizer-cli" \
  "$APP_DIR/src-tauri/resources/bin/summarizer-cli"
