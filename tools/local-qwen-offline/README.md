# Native Local Qwen Agent for Warp (macOS)

This branch adds a native in-process local Agent Mode backend. In WARP_LOCAL_OFFLINE=1 mode, app/src/ai/agent/api/impl.rs routes requests to app/src/ai/agent/api/local.rs instead of Warp's remote MAA endpoint. The backend sends OpenAI Chat Completions requests directly to a loopback-only MLX OptiQ endpoint and returns native Warp task/tool events.

## Start

1. Start your local MLX OptiQ server and confirm http://127.0.0.1:9163/v1/models responds.
2. From the root of this checkout, build the fork once with locally cached dependencies:

```bash
cargo build --offline -p warp --bin warp
```

3. Launch through the network-isolating wrapper:

```bash
chmod +x tools/local-qwen-offline/run-local-qwen.sh
tools/local-qwen-offline/run-local-qwen.sh
```

Useful environment overrides:

```bash
OPTIQ_BASE_URL='http://127.0.0.1:9163/v1' \
OPTIQ_MODEL_ID='your-exact-model-id-from-v1-models' \
tools/local-qwen-offline/run-local-qwen.sh
```

OPTIQ_API_KEY is optional. By default, the local backend discovers the model ID using GET /v1/models; if discovery fails, it falls back to mlx-community/Qwen3.5-9B-OptiQ-4bit. Set OPTIQ_MODEL_ID if the ID advertised by your server differs.

## What is implemented

- Native local MAA request routing to OptiQ (no OpenCode process).
- OpenAI Chat Completions tool calling for local shell commands, file reads, file creation/editing, grep, file glob, and local codebase search.
- Tool calls are emitted as normal Warp MAA task events so the existing local action UI and approvals handle the actual operation.
- Computer use requests go through Warp's permission flow. The local model receives approved screenshot bytes through the loopback request, and computer actions are routed through Warp's existing executor.
- Web search, research agent, orchestration, and cloud-subagent tools are not offered by the local backend.
- Telemetry event macros do not enqueue in offline mode; the telemetry collector skips flush/persistence and discards queued telemetry.
- The launcher wraps the process in a macOS Seatbelt policy that denies external network access and allows loopback only.

## Important limitations and validation

This is a first native prototype, not a verified production release. I cannot run the repository's Rust build or macOS desktop/security tests from GitHub's hosted source editor. Run cargo check -p warp --bin warp and the manual test below on the target Mac before relying on it.

The local backend is non-streaming: the model's answer appears after each Chat Completions request finishes. It supports the tools explicitly implemented in local.rs; unsupported Warp tool types are not exposed. Tool results are fed back on the next turn, matching Warp's existing request/action-result lifecycle.

Computer use must be approved in Warp first. This branch does not launch cloud subagents. A model can produce bad tool arguments; the existing Warp tool executor and permission UI must remain enabled, and shell commands/file changes should be reviewed.

The launcher uses macOS sandbox-exec, which Apple has deprecated and may remove in a future macOS release. The script fails closed if the command is absent and checks that OptiQ is reachable locally and an external HTTP probe does not succeed. This is additional containment, not a substitute for a security review. For a strong no-egress guarantee, verify the process tree with a packet capture or outbound firewall rule as well.

## Manual test checklist

1. Start OptiQ, then run the launcher.
2. Ask the Warp Agent to read a small local file.
3. Ask it to create a new file; confirm the diff is presented by Warp.
4. Ask it to run pwd or ls and confirm Warp handles the command.
5. Request computer control; approve it in Warp, then test a harmless mouse move/screenshot.
6. With the app still running, disconnect the network. The local Agent should continue working. Web search and cloud agents should be unavailable.
7. Confirm an external connection attempt is blocked by the Seatbelt profile.

A build/test failure or a missing tool mapping should be fixed before using this on proprietary repositories.
