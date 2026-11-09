#!/usr/bin/env bash
# Build and run terminal-cam (release). Extra args pass through,
# e.g. ./run.sh --virtual-cam
set -euo pipefail
cd "$(dirname "$0")"

if ! command -v cargo >/dev/null 2>&1; then
  [ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
fi
command -v cargo >/dev/null 2>&1 || {
  echo "cargo not found — install Rust: https://rustup.rs" >&2
  exit 1
}

exec cargo run --release -- "$@"
