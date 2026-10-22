#!/bin/bash
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
PROFILE_TEMPLATE="$ROOT/tools/local-qwen-offline/offline.sb"
APP_BIN="${WARP_BIN:-$ROOT/target/debug/warp}"
RAW_OPTIQ_BASE_URL="${OPTIQ_BASE_URL:-http://127.0.0.1:9163/v1}"

fail() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }

[[ "$(uname -s)" == "Darwin" ]] || fail "This launcher requires macOS."
command -v sandbox-exec >/dev/null 2>&1 || fail "sandbox-exec is unavailable; refusing to launch without an OS network sandbox."
command -v curl >/dev/null 2>&1 || fail "curl is required."
command -v python3 >/dev/null 2>&1 || fail "python3 is required."
[[ -x "$APP_BIN" ]] || fail "Warp binary not found at $APP_BIN. Build first with: cargo build --offline -p warp --bin warp"

# Parse the endpoint structurally; never rely on a string-prefix check, which could
# accidentally accept a URL with userinfo (127.0.0.1:port@external.example).
OPTIQ_ENDPOINT_INFO="$(RAW_OPTIQ_BASE_URL="$RAW_OPTIQ_BASE_URL" python3 - <<'PY'
import os
import sys
from urllib.parse import urlsplit

try:
    value = os.environ["RAW_OPTIQ_BASE_URL"].strip()
    url = urlsplit(value)
    port = url.port or 80
except (KeyError, ValueError) as exc:
    print(f"Invalid OPTIQ_BASE_URL: {exc}", file=sys.stderr)
    sys.exit(2)

if url.scheme != "http" or url.hostname != "127.0.0.1":
    print("OPTIQ_BASE_URL must use plain HTTP and host 127.0.0.1 only.", file=sys.stderr)
    sys.exit(2)
if url.username is not None or url.password is not None or url.query or url.fragment:
    print("OPTIQ_BASE_URL must not contain credentials, query parameters, or fragments.", file=sys.stderr)
    sys.exit(2)
if url.path.rstrip("/") not in ("", "/v1"):
    print("OPTIQ_BASE_URL path must be empty, '/', or '/v1'.", file=sys.stderr)
    sys.exit(2)

authority = "127.0.0.1" + (f":{port}" if port != 80 else "")
print(f"http://{authority}/v1\t{port}")
PY
)" || fail "Refusing an invalid OPTIQ_BASE_URL."
IFS=$'\t' read -r OPTIQ_BASE_URL OPTIQ_PORT <<< "$OPTIQ_ENDPOINT_INFO"

# Materialize a per-run policy whose only allowed network destination is OptiQ's
# exact loopback port. No DNS, other local ports, or remote hosts are permitted.
PROFILE="$(mktemp -t warp-local-qwen-offline)" || fail "Could not create temporary Seatbelt policy."
trap 'rm -f "$PROFILE"' EXIT
python3 - "$PROFILE_TEMPLATE" "$PROFILE" "$OPTIQ_PORT" <<'PY'
import pathlib
import sys

source_path, output_path, port_text = sys.argv[1:]
port = int(port_text)
if not (1 <= port <= 65535):
    raise SystemExit("OptiQ port must be between 1 and 65535.")
source = pathlib.Path(source_path).read_text(encoding="utf-8")
token = "OPTIQ_PORT_PLACEHOLDER"
if source.count(token) != 1:
    raise SystemExit("offline.sb must contain exactly one OptiQ port placeholder.")
pathlib.Path(output_path).write_text(source.replace(token, str(port)), encoding="utf-8")
PY
[[ -s "$PROFILE" ]] || fail "Could not create the Seatbelt network policy."

MODELS_URL="$OPTIQ_BASE_URL/models"

# Confirm local MLX OptiQ is reachable under the same policy as Warp.
sandbox-exec -f "$PROFILE" /usr/bin/curl --noproxy '*' --fail --silent --show-error --max-time 3 "$MODELS_URL" >/dev/null \
  || fail "Local OptiQ is not reachable at $MODELS_URL under the offline sandbox. Start mlx-optiq first; Warp was not started."

# Fail closed if an external IPv4 connection unexpectedly succeeds under the same policy.
EXTERNAL_STATUS="$(sandbox-exec -f "$PROFILE" /usr/bin/curl --noproxy '*' --connect-timeout 2 --max-time 3 --silent --output /dev/null --write-out '%{http_code}' http://1.1.1.1/cdn-cgi/trace 2>/dev/null || true)"
if [[ -n "$EXTERNAL_STATUS" && "$EXTERNAL_STATUS" != "000" ]]; then
  fail "External network is reachable from the sandbox (HTTP $EXTERNAL_STATUS). Warp was not started."
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

printf '\nNative local Warp Agent\n'
printf '  OptiQ endpoint: %s\n' "$OPTIQ_BASE_URL"
printf '  Model ID:       %s (auto-discovered from /v1/models unless OPTIQ_MODEL_ID is set)\n' "${OPTIQ_MODEL_ID:-auto}"
printf '  Network:        127.0.0.1:%s only; external connections denied by macOS sandbox\n' "$OPTIQ_PORT"
printf '  Web search:     disabled\n'
printf '  Warp telemetry: disabled; queued telemetry is discarded\n\n'

sandbox-exec -f "$PROFILE" /usr/bin/env \
  WARP_LOCAL_OFFLINE=1 \
  OPTIQ_BASE_URL="$OPTIQ_BASE_URL" \
  OPTIQ_API_KEY="${OPTIQ_API_KEY:-}" \
  OPTIQ_MODEL_ID="${OPTIQ_MODEL_ID:-}" \
  NO_PROXY='*' no_proxy='*' \
  HTTP_PROXY='' HTTPS_PROXY='' ALL_PROXY='' \
  http_proxy='' https_proxy='' all_proxy='' \
  "$APP_BIN" "$@"
