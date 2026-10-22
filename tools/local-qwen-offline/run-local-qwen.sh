#!/bin/bash
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
PROFILE="$ROOT/tools/local-qwen-offline/offline.sb"
APP_BIN="${WARP_BIN:-$ROOT/target/debug/warp}"
OPTIQ_BASE_URL="${OPTIQ_BASE_URL:-http://127.0.0.1:9163/v1}"

fail() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }

[[ "$(uname -s)" == "Darwin" ]] || fail "This launcher requires macOS."
command -v sandbox-exec >/dev/null 2>&1 || fail "sandbox-exec is unavailable; refusing to launch without an OS network sandbox."
command -v curl >/dev/null 2>&1 || fail "curl is required."
[[ -x "$APP_BIN" ]] || fail "Warp binary not found at $APP_BIN. Build first with: cargo build --offline -p warp --bin warp"

case "$OPTIQ_BASE_URL" in
  http://127.0.0.1:*|http://localhost:*|http://\[::1\]:*) ;;
  *) fail "OPTIQ_BASE_URL must use localhost/127.0.0.1/[::1]; refusing remote endpoints." ;;
esac

MODELS_URL="${OPTIQ_BASE_URL%/}/models"
curl --noproxy '*' --fail --silent --show-error --max-time 3 "$MODELS_URL" >/dev/null \
  || fail "OptiQ is not responding at $MODELS_URL. Start mlx-optiq first."

sandbox-exec -f "$PROFILE" /usr/bin/curl --noproxy '*' --fail --silent --show-error --max-time 3 "$MODELS_URL" >/dev/null \
  || fail "The OS sandbox cannot reach local OptiQ. Warp was not started."

EXTERNAL_STATUS="$(sandbox-exec -f "$PROFILE" /usr/bin/curl --noproxy '*' --connect-timeout 2 --max-time 3 --silent --output /dev/null --write-out '%{http_code}' http://1.1.1.1/cdn-cgi/trace 2>/dev/null || true)"
if [[ -n "$EXTERNAL_STATUS" && "$EXTERNAL_STATUS" != "000" ]]; then
  fail "External network is reachable from the sandbox (HTTP $EXTERNAL_STATUS). Check offline.sb; Warp was not started."
fi

export WARP_LOCAL_OFFLINE=1
export OPTIQ_BASE_URL
export NO_PROXY='*'
export no_proxy='*'
export HTTP_PROXY=''
export HTTPS_PROXY=''
export ALL_PROXY=''
export http_proxy=''
export https_proxy=''
export all_proxy=''

printf '\nLocal-only Warp Agent\n'
printf '  OptiQ endpoint: %s\n' "$OPTIQ_BASE_URL"
printf '  Model ID:       %s (auto-discovered from /v1/models unless OPTIQ_MODEL_ID is set)\n' "${OPTIQ_MODEL_ID:-auto}"
printf '  Network:        loopback only; external connections denied by sandbox\n'
printf '  Web search:     disabled in local MAA request\n'
printf '  Warp telemetry: disabled and queued telemetry is discarded\n'
printf '\nThis starts the native local backend in this fork, not OpenCode.\n'
printf 'All network access is blocked except loopback; computer-use requires Warp approval.\n\n'

exec sandbox-exec -f "$PROFILE" /usr/bin/env \
  WARP_LOCAL_OFFLINE=1 \
  OPTIQ_BASE_URL="$OPTIQ_BASE_URL" \
  OPTIQ_API_KEY="${OPTIQ_API_KEY:-}" \
  OPTIQ_MODEL_ID="${OPTIQ_MODEL_ID:-}" \
  NO_PROXY='*' no_proxy='*' \
  HTTP_PROXY='' HTTPS_PROXY='' ALL_PROXY='' \
  http_proxy='' https_proxy='' all_proxy='' \
  "$APP_BIN" "$@"
