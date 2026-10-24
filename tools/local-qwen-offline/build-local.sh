#!/usr/bin/env bash
# Minimal offline build: no Homebrew, Xcode install, Cargo install, model download,
# dependency fetch, app bundling, telemetry setup, or network access.
set -Eeuo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "ERROR: This helper is intended for macOS." >&2
  exit 2
fi
if [[ "$(uname -m)" != "arm64" ]]; then
  echo "ERROR: Expected Apple Silicon (arm64); found $(uname -m)." >&2
  exit 2
fi
if ! command -v xcode-select >/dev/null 2>&1 || ! xcode-select -p >/dev/null 2>&1; then
  echo "ERROR: Xcode/Command Line Tools are not configured. This script will not install them." >&2
  exit 2
fi
if ! command -v rustup >/dev/null 2>&1; then
  echo "ERROR: rustup is missing. This script will not install Rust." >&2
  exit 2
fi
if ! rustup toolchain list | grep -Eq '^1\.92\.0([ -]|$)'; then
  echo "ERROR: pinned Rust 1.92.0 is not installed." >&2
  echo "Install it explicitly once, when you choose to allow the download:" >&2
  echo "  rustup toolchain install 1.92.0 --profile minimal" >&2
  exit 2
fi

echo "Building warp-oss with pinned Rust 1.92.0 and Cargo offline mode."
echo "No dependencies will be downloaded. If the local cache is incomplete, Cargo will fail and this script will stop."
rustup run 1.92.0 cargo build --offline --locked -p warp@0.1.0 --bin warp-oss

BIN="$ROOT/target/debug/warp-oss"
if [[ -x "$BIN" ]]; then
  echo
  echo "Build succeeded: $BIN"
  ls -lh "$BIN"
else
  echo "ERROR: Cargo reported success but the expected binary was not found: $BIN" >&2
  exit 1
fi

echo
echo "This helper only builds the binary. It does not launch it, package a .app, or claim network isolation."
