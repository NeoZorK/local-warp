# Local Qwen Offline Mode (macOS)

This support package provides a **fail-closed offline launcher** for the Warp source build and configures OpenCode as a local ACP agent backed by MLX OptiQ.

## Security behavior

- Requires macOS `sandbox-exec`; it refuses to launch if the OS sandbox is unavailable.
- The sandbox denies network operations by default and allows outbound connections only to loopback (for example, `127.0.0.1:9163`) plus local Unix sockets.
- It probes the OptiQ `/v1/models` endpoint through the same sandbox and aborts if an external HTTP probe succeeds.
- Sets `WARP_LOCAL_OFFLINE=1`. The Warp telemetry macros stop enqueueing events; the telemetry collector clears in-memory/persisted queues and does not flush or save telemetry in this mode.
- OpenCode is configured with `websearch=false`, `webfetch=deny`, and `task=deny`; external providers are disabled. Shell commands require approval.
- Model requests are sent to the local OptiQ server, not to Warp's inference API.

## Requirements

1. A locally built Warp binary at `target/debug/warp` (or set `WARP_BIN` to the exact binary path).
2. `mlx-optiq` running on the Mac and serving `http://127.0.0.1:9163/v1/models`.
3. OpenCode installed. Its ACP adapter is enabled in this fork by `app/src/ai/agent_sdk/driver/harness/acp/manifest.rs`.
4. Dependencies already cached for offline builds. Build once with `cargo build --offline -p warp` before launching.

## Run

From the repository root:

```bash
chmod +x tools/local-qwen-offline/run-local-qwen.sh
tools/local-qwen-offline/run-local-qwen.sh
```

If OptiQ's model ID cannot be discovered, set it explicitly:

```bash
OPTIQ_MODEL_ID='your-exact-id-from-v1-models' tools/local-qwen-offline/run-local-qwen.sh
```

You can customize the local API address (it must remain loopback):

```bash
OPTIQ_BASE_URL='http://127.0.0.1:9163/v1' tools/local-qwen-offline/run-local-qwen.sh
```

## Important scope limitation

This patch **does not yet replace Warp's first-party Agent Mode MAA loop with a native in-process OptiQ client**. Warp's native path still constructs requests for the remote `/ai/multi-agent` service. Under this launch profile those requests cannot leave the machine, so the first-party cloud agent will not function offline. Use the OpenCode ACP agent/harness inside Warp for local model-driven work.

Likewise, Warp's native Computer Use executor is part of the first-party MAA tool loop. It is not automatically available to an OpenCode ACP turn. A fully native local implementation must explicitly bridge local Qwen tool calls into Warp's existing local action executor and map vision/screenshot payloads; that bridge is not implemented by this support package. Do not treat this prototype as satisfying that requirement.

`sandbox-exec` is deprecated by Apple. The script fails closed if it is missing and performs network probes before launching, but this repository change has not been built or security-tested on your Mac. For the strongest assurance, additionally monitor/block outbound traffic at the OS firewall and independently verify the process tree.
