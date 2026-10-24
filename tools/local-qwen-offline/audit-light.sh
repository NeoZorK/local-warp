#!/usr/bin/env bash
# Read-only, offline dependency/build-size inventory for the Local Warp fork.
# This script does not install tools, fetch dependencies, modify source, or push anything.
set -Eeuo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

if ! command -v rustup >/dev/null 2>&1; then
  echo "ERROR: rustup is missing. This audit script will not install Rust." >&2
  exit 127
fi
if ! rustup toolchain list | grep -Eq '^1\.92\.0([ -]|$)'; then
  echo "ERROR: pinned Rust 1.92.0 is not installed; stopping before calling any rustup shim." >&2
  echo "Install it explicitly only if you choose to allow the download:" >&2
  echo "  rustup toolchain install 1.92.0 --profile minimal" >&2
  exit 2
fi
if ! command -v git >/dev/null 2>&1; then
  echo "ERROR: git is not installed." >&2
  exit 127
fi
cargo_cmd() { rustup run 1.92.0 cargo "$@"; }
rustc_cmd() { rustup run 1.92.0 rustc "$@"; }

STAMP="$(date '+%Y%m%d-%H%M%S')"
OUT="$ROOT/audit/local-light-$STAMP"
mkdir -p "$OUT"

run_capture() {
  local name="$1"
  shift
  echo "==> $*" | tee "$OUT/$name.log"
  set +e
  "$@" 2>&1 | tee -a "$OUT/$name.log"
  local rc=${PIPESTATUS[0]}
  set -e
  printf '\nexit_code=%s\n' "$rc" | tee -a "$OUT/$name.log"
  return 0
}

{
  echo "repo=$ROOT"
  echo "timestamp=$STAMP"
  echo "branch=$(git branch --show-current 2>/dev/null || true)"
  echo "commit=$(git rev-parse HEAD 2>/dev/null || true)"
  echo "rustc=$(rustc_cmd --version 2>&1 || true)"
  echo "cargo=$(cargo_cmd --version 2>&1 || true)"
  echo "macOS=$(sw_vers -productVersion 2>/dev/null || true)"
  echo "architecture=$(uname -m)"
  echo
  echo "[git status]"
  git status --short
} > "$OUT/environment.txt"

run_capture cargo-tree-normal cargo_cmd tree --offline -p warp@0.1.0 --edges normal
run_capture cargo-tree-features cargo_cmd tree --offline -p warp@0.1.0 --edges features
run_capture cargo-tree-duplicates cargo_cmd tree --offline -p warp@0.1.0 --duplicates
run_capture cargo-metadata cargo_cmd metadata --offline --format-version 1 --no-deps

# Reverse-dependency probes are best-effort: a missing package is recorded, not installed.
for package in aws-config aws-sdk-sts aws-sdk-sso aws-sdk-ssooidc aws-sdk-signin sentry opentelemetry-otlp; do
  safe_name="$(printf '%s' "$package" | tr -c '[:alnum:]_.-' '_')"
  run_capture "reverse-$safe_name" cargo_cmd tree --offline -p warp@0.1.0 --invert "$package"
done

{
  echo "[repository and target sizes]"
  du -sh . 2>/dev/null || true
  du -sh target 2>/dev/null || echo "target/: absent"
  echo
  echo "[built application bundles]"
  find target -maxdepth 6 -type d -name '*.app' -print 2>/dev/null || true
  echo
  echo "[largest target subdirectories]"
  if [ -d target ]; then
    du -sh target/* 2>/dev/null | sort -h | tail -30 || true
  fi
} > "$OUT/sizes.txt"

cat > "$OUT/README.txt" <<'EOF'
This is a read-only, offline inventory. Cargo commands use --offline and may fail if the local dependency cache is incomplete.
A non-zero cargo-tree exit code means the inventory is incomplete; it does not prove a dependency is unused.
No network fetch or installation was attempted.
EOF

echo
echo "Audit saved to: $OUT"
echo "Review environment.txt, sizes.txt, and each cargo-tree-*.log."
echo "If any log reports missing cached packages, stop there; this script intentionally does not fetch them."
