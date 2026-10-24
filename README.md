# Local Warp — local-first terminal fork

An independent macOS-focused fork of the open-source Warp terminal client, being adapted to run local AI features through on-device Apple Foundation Models and a local MLX OptiQ endpoint (Qwen 9B). The goal is a lightweight terminal with local MCP, local codebase search/indexing, and no required Warp account or hosted AI service.

**This is an independent community fork. It is not an official Warp product and is not endorsed by Warp or Denver Technologies, Inc.** “Warp” is used here only to identify the upstream project from which this code is derived.

## Project status

**Experimental — not yet validated as a stable release.** The local Agent prototype exists, but the repository still contains substantial upstream cloud/account code and dependencies. Do not assume that a build is fully offline until the validation checklist below passes on the target Mac.

## Intended scope

- Native macOS terminal, tabs, command blocks, shell execution, and local settings.
- Local AI providers only: Apple Foundation Models running on-device and MLX OptiQ through a loopback OpenAI-compatible API.
- Local MCP servers and existing permission-gated local file/shell actions.
- Local codebase search and indexing, subject to audit of their runtime network paths.
- No required registration, email login, cloud sync, billing, referrals, hosted agents, or web search in the finished local-only profile.

## Local Qwen prototype

The current prototype expects OptiQ at `http://127.0.0.1:9163/v1` by default. Start OptiQ separately and check its `/v1/models` endpoint before testing.

From a checkout with all Rust dependencies already cached:

```bash
WARP_LOCAL_OFFLINE=1 cargo run --offline -p warp --bin warp-oss
```

The current native local Agent launcher and sandbox profile are experimental. See [the prototype notes](tools/local-qwen-offline/README.md). Do not use the app for sensitive work until the build, network containment, local tool permissions, and no-egress checks have passed on the target machine.

## Development and audit

- [Lightweight refactor audit and staged removal plan](docs/LOCAL_LIGHT_AUDIT.md)
- [Local Qwen prototype notes](tools/local-qwen-offline/README.md)

The audit script is read-only and uses Cargo's offline mode. It does not install toolchains, download dependencies, alter source files, or push changes:

```bash
bash tools/local-qwen-offline/audit-light.sh
```

Use the Rust version pinned in `rust-toolchain.toml`. Install only the Rust components required by the build/test command you are actually running; `rust-analyzer` is for editor integration, not required to compile the app.

## Licensing and attribution

This repository is a derivative of the open-source Warp client. Preserve the upstream license files and notices:

- Most source code is under GNU Affero General Public License v3.0-only; see [LICENSE-AGPL](LICENSE-AGPL).
- The `warpui` and `warpui_core` UI framework crates are under the MIT License; see [LICENSE-MIT](LICENSE-MIT).
- Third-party code and assets may have separate licenses. Keep the existing per-component notices and regenerate the third-party license inventory when dependencies change.

Do not relicense the entire repository as MIT. When distributing a modified build, follow the applicable license obligations, including providing the corresponding source where required. This README is project documentation, not legal advice.

## Security and privacy boundary

“Local-only” refers to the app's own AI/service requests; it does not prevent a user from deliberately running network-capable shell commands such as `curl`, `ssh`, `git fetch`, or package managers. The finished strict mode must block or fail closed on app-managed non-loopback service calls and must be verified with an outbound firewall or packet-level test. A source flag alone is not proof that no data leaves the machine.

## Upstream

Derived from [warpdotdev/warp](https://github.com/warpdotdev/warp). This fork maintains its own roadmap and should not be represented as the official Warp app.
