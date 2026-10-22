#!/bin/bash
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
PROFILE="$ROOT/tools/local-qwen-offline/offline.sb"
APP_BIN="${WARP_BIN:-$ROOT/target/debug/warp}"
OPTIQ_BASE_URL="${OPTIQ_BASE_URL:-http://127.0.0.1:9163/v1}"
CONFIG_DIR="${HOME}/.config/warp-local-qwen"
CONFIG="$CONFIG_DIR/opencode.json"

fail() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }

[[ "$(uname -s)" == "Darwin" ]] || fail "This launcher requires macOS."
command -v sandbox-exec >/dev/null 2>&1 || fail "sandbox-exec is unavailable; refusing to start without an OS network sandbox."
command -v curl >/dev/null 2>&1 || fail "curl is required."
command -v python3 >/dev/null 2>&1 || fail "python3 is required."
command -v opencode >/dev/null 2>&1 || fail "OpenCode is required for the local ACP route."
[[ -x "$APP_BIN" ]] || fail "Warp binary not found at $APP_BIN. Build the fork first with: cargo build --offline -p warp"

# Query only loopback; this is not a cloud request.
MODELS_JSON="$(curl --noproxy '*' --fail --silent --show-error --max-time 3 "$OPTIQ_BASE_URL/models")" || fail "OptiQ is not responding at $OPTIQ_BASE_URL. Start mlx-optiq locally first."
MODEL_ID="${OPTIQ_MODEL_ID:-$(printf '%s' "$MODELS_JSON" | python3 -c 'import json,sys; d=json.load(sys.stdin); a=d.get("data", []); print(a[0].get("id", "") if a else "")')}"
[[ -n "$MODEL_ID" ]] || fail "Could not discover a model ID from /v1/models. Set OPTIQ_MODEL_ID explicitly."

mkdir -p "$CONFIG_DIR"
chmod 700 "$CONFIG_DIR"
export WARP_LOCAL_OFFLINE=1
export OPENCODE_DISABLE_AUTOUPDATE=1
export OPENCODE_DISABLE_TELEMETRY=1
export OPENCODE_MODEL="local-qwen/qwen-9b"
export OPTIQ_BASE_URL MODEL_ID CONFIG

# Isolate OpenCode from other user/project configurations and explicitly disable online tools.
python3 <<'PY'
import json, os
model_id = os.environ["MODEL_ID"]
base_url = os.environ["OPTIQ_BASE_URL"]
config = {
    "$schema": "https://opencode.ai/config.json",
    "model": "local-qwen/qwen-9b",
    "providers": {
        "local-qwen": {
            "name": "Local MLX OptiQ",
            "package": "@opencode/ai/providers/openai-compatible",
            "settings": {"baseURL": base_url},
            "models": {
                "qwen-9b": {
                    "name": "Qwen 9B MLX OptiQ",
                    "modelID": model_id,
                    "capabilities": {
                        "tools": True,
                        "input": ["text", "image"],
                        "output": ["text"]
                    },
                    "limit": {"context": 32768, "output": 8192}
                }
            }
        }
    },
    "websearch": False,
    "disabled_providers": [
        "openai", "anthropic", "google", "openrouter", "xai",
        "opencode", "amazon-bedrock", "azure", "mistral", "groq",
        "deepseek", "togetherai"
    ],
    "permission": {
        "websearch": "deny",
        "webfetch": "deny",
        "task": "deny",
        "shell": "ask",
        "edit": "allow",
        "write": "allow",
        "read": "allow",
        "external_directory": "ask"
    }
}
path = os.environ["CONFIG"]
with open(path, "w", encoding="utf-8") as f:
    json.dump(config, f, indent=2)
    f.write("\n")
os.chmod(path, 0o600)
PY

# Ensure loopback access works under the same kernel policy that will wrap Warp.
sandbox-exec -f "$PROFILE" /usr/bin/curl --noproxy '*' --fail --silent --show-error --max-time 3 "$OPTIQ_BASE_URL/models" >/dev/null || fail "The OS sandbox could not reach local OptiQ. No application was launched."

# Fail closed if an external HTTP connection succeeds from the same sandbox.
PROBE_CODE="$(sandbox-exec -f "$PROFILE" /usr/bin/curl --noproxy '*' --connect-timeout 2 --max-time 3 --silent --output /dev/null --write-out '%{http_code}' http://1.1.1.1/cdn-cgi/trace 2>/dev/null || true)"
if [[ -n "$PROBE_CODE" && "$PROBE_CODE" != "000" ]]; then
  fail "External network probe succeeded (HTTP $PROBE_CODE); refusing to launch in an insecure state."
fi

cat <<EOF
Starting Warp with OS-enforced offline networking.
Allowed: localhost (for OptiQ) and local Unix sockets.
Blocked: external network, Warp servers, telemetry uploads, web search.
Model: $MODEL_ID
OpenCode config: $CONFIG

Note: this route enables OpenCode's ACP harness in Warp. It does not replace Warp's
first-party cloud MAA backend with a native local MAA implementation.
EOF

export OPENCODE_CONFIG="$CONFIG"
# Inline config has highest precedence; keep all online tools disabled in project directories too.
export OPENCODE_CONFIG_CONTENT='{"websearch":false,"permission":{"websearch":"deny","webfetch":"deny","task":"deny"}}'
exec sandbox-exec -f "$PROFILE" "$APP_BIN" "$@"
