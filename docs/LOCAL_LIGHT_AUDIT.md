# Local-first/light refactor audit

**Audit baseline:** branch `local-qwen-offline-prototype`, commit `5acb2066d80b74c173d25cefcea4d319e904dae9` (2026-10-23).  
**Working branch:** `light-audit-phase1`.  
**Status:** source-level inventory only; no successful macOS build has yet been recorded.

This document separates observed source structure from planned removals. A crate is not safe to delete just because its name looks cloud-related: first inspect reverse dependencies and feature gates, then remove references, compile, and test.

## 0. Change inventory and exact rollback anchors

The local-Qwen prototype was added in a sequence of small commits on 2026-10-22. These are the feature groups currently present in the branch:

| Date (UTC) | Commit(s) | What was added or changed |
|---|---|---|
| 2026-10-22 21:33 | `5d1f3501b97b`, `14da4c88516b`, `0ce859c073cf` | Native in-process OptiQ Agent backend; register it and route Agent Mode to local OptiQ in offline mode. |
| 2026-10-22 21:33–21:35 | `767ccf2e304c`, `f3c4b5492967`, `7e27b96f87a0`, `f35a1c19196b`, `311d1d8fec80` | Remove unrelated OpenCode ACP wiring; use a direct native launcher; document prototype; harden endpoint validation and OptiQ compatibility. |
| 2026-10-22 21:40–21:43 | `0887559a3d85`, `66d1b893dab0`, `234bf1f4e8a1`, `85b81561f95e`, `4fdd8ae23eef`, `73e43651a488`, `a88c03559a92`, `09f2afcce2d2`, `3ed69d635c93`, `0b9e24274a6f`, `04f60c2ffa6c` | Local Qwen model metadata and picker; local model labels; loopback endpoint restrictions; local-only model selection; context formatting and iterator fixes. |
| 2026-10-22 21:45–21:47 | `cfcff4b96b89`, `b5a058c47aab`, `fc274bf49c9a`, `ac02069107f5`, `b865654f9e14`, `d8ff2fd5e23a` | Block non-OptiQ HTTP/SSE; restrict sandbox to OptiQ's port; scope computer-use approvals to the task; disable redirects in offline mode; disallow remote terminal sessions for local Agent. |
| 2026-10-22 21:52–21:54 | `a8e6f1ac469b`, `8f4cc4420170`, `e574bf75df5f`, `bbd52e153e85`, `298762b23e2e`, `6fd938fd181f` | Iterative local tool loop and action-result feedback so the model can receive completed tool results and continue. |
| 2026-10-22 21:54–22:02 | `faaff3b5e4bf`, `b7fcc3a0a720`, `0cf631cd7c8b`, `4b0e8c59cab2`, `95f48016e06d`, `fe189df9d689`, `2de3c743fd61`, `d0f8aaba8b1a`, `f883cc715a1a` | Computer-use input validation and history bounds; OSS launch target; avoid duplicate cloud-style follow-ups; avoid stale stream cleanup bugs. |
| 2026-10-22 22:06–22:10 | `f46ac9e1fee8`, `46b5b931f6ee`, `2ed2b01dd222`, `505b6e3fa3fa`, `15087fff5dd4`, `68939ad20696` | Further computer-use/offline-sandbox safeguards, tool-call ordering and pending-result cleanup. |
| 2026-10-23 12:43 | `5acb2066d80b` | Gitleaks secret-scanning workflow for this branch. |
| 2026-10-24 12:20–12:22 | `65f1d3bb6216`, `1f662918a4ce`, `2e602331f9d6`, `3d324d95f75b`, `b4d4242e89be`, `eb306287ca6c`, `7bdc0abcac53`, `448ea2741603` | Current light-refactor phase: narrow compile fix; independent README; this audit; read-only dependency inventory; minimal pinned toolchain; offline build helper; macOS compile-check workflow. |

### Rollback points

- **Current local-Qwen prototype before the light refactor:** `5acb2066d80b74c173d25cefcea4d319e904dae9`.
- **Immediately before the local-Qwen feature was introduced:** `6b9c6a8cc70a19ccfc0f1259aca0f521bb6b3d25` (parent of the first local Agent commit `5d1f3501b97b`).
- **Current light-refactor work:** branch `light-audit-phase1`, latest compile-check workflow commit `448ea274160381dc10eb86bd252bdcd638f3d6bb`.

To inspect the pre-local baseline without changing the current checkout, create a separate branch/worktree from the exact commit:

```bash
git switch -c inspect/pre-local-qwen 6b9c6a8cc70a19ccfc0f1259aca0f521bb6b3d25
```

To return to the full local-Qwen prototype, switch to `local-qwen-offline-prototype`. To continue the current audit/refactor, switch to `light-audit-phase1`. Prefer switching branches over `reset --hard` or force-pushing. The local-Qwen additions are interdependent; do not revert one middle commit in isolation without rebuilding and testing.

## 1. Current known blocker

In `crates/http_client/src/lib.rs`, the offline SSE-denial branch used `cfg_if!` as an expression to return target-specific boxed streams. The compiler log reported a macro-position/type mismatch there. Phase 1 replaces that return expression with target-specific `#[cfg]` blocks. This is a narrow source fix; it is **not yet verified by a native macOS build**.

## 2. Keep as product-critical

| Area | Relevant code/crates observed | Decision |
|---|---|---|
| Terminal and shell | `warp_terminal`, `command`, `warp_completer`, `warp_util` | Keep |
| UI | `warpui`, `warpui_core`, `ui_components`, `warpui_extras` | Keep UI foundations; remove only cloud/account-specific screens and models |
| Files and editor | `editor` / `warp_editor`, `warp_files`, `virtual-fs`, `syntax_tree`, `languages` | Keep |
| Local AI | `app/src/ai/agent/api/local.rs`, local provider adapter, local settings | Keep; expose Apple on-device and loopback OptiQ only |
| Local tool execution | `computer_use`, local shell/file actions, permission/approval flow | Keep, with approvals and explicit local-session guard |
| MCP | `mcp`, `rmcp`, local MCP configuration | Keep local stdio/loopback MCP; remove hosted OAuth/catalog paths if not needed |
| Codebase search/index | `warp_search_core`, `warp_ripgrep`, `repo_metadata`, `watcher`, relevant parser/LSP crates | Keep; audit every runtime network call |
| Local persistence | `persistence`, `settings`, `settings_value`, local history | Keep local storage; remove remote sync |
| Network client | `http_client` | Keep for loopback OptiQ and local MCP; add fail-closed host policy, don't delete wholesale |

## 3. First removal candidates (not yet deleted)

These are strong candidates based on workspace dependency names and the product requirements. Confirm usage with `cargo tree -i <crate>` and source search before removal.

### Product surfaces to remove from UI/routes
- Team, organization and workspace membership flows.
- Sign-up, login, email/account linking, subscription, billing, token/credit purchase, referrals and usage metering.
- Settings sync, Warp Drive cloud object sync, shared cloud blocks and session sharing.
- Hosted Agent/Oz/orchestration, cloud subagents, cloud workflows/notebooks and managed remote secrets.
- Claude/hosted model-provider flows and web/research search tools.
- Telemetry opt-in/opt-out surfaces that exist only for hosted service collection, crash-report uploads, usage flushing, remote changelog/update polling and remote fallback asset fetches.

### Workspace crates to investigate for complete removal
- `cloud_object_client`, `cloud_object_models`, `cloud_objects`, `cloud_object_persistence`
- `firebase`
- `warp_graphql`, `warp_graphql_schema` (check if any local UI data still uses them)
- `warp_harness_usage`
- `warp_server_auth`, `warp_server_client`
- `warp_multi_agent_client`, `warp_multi_agent_api` (local prototype currently reuses the API event schema; first extract/retain the minimal local event types)
- `warp_workflows` and related workflow types (check exact package names and reverse dependencies)
- `remote_server` (preserve manually launched SSH and ordinary shell workflows; remove only Warp-managed remote server download/configuration)
- `warp_managed_secrets` (preserve local environment variables and local secret handling where useful)
- `websocket` (remove only if session-sharing or other remaining local feature does not depend on it)
- `voice_input` (optional and not in the two-provider scope)
- Sentry/crash upload and OpenTelemetry/telemetry transport dependencies, once the app has no remaining compile-time dependency on their types.

**AWS SDK dependencies:** do not remove individual `aws-*` packages by hand. They are transitive; first use `cargo tree --offline -i aws-config` (and the exact package names reported by Cargo) to identify which remaining crate pulls them in. Removing the cloud dependency root should remove the SDK transitively if no other feature needs it.

## 4. Build-tool simplification

- Keep `rust-toolchain.toml` pinned at Rust `1.92.0` until the project is known to build with a different version. Avoid installing both an arbitrary latest stable and the pinned toolchain.
- `rustfmt`, `clippy`, and `rust-analyzer` are declared in the toolchain file. `rust-analyzer` is for editor integration, not compilation; remove it from the automatic setup path, not necessarily from the developer toolchain declaration. Run formatting/lint only when requested.
- Don't install `cargo-binstall` or `cargo-bundle` in the first test loop. First build the existing `warp-oss` binary; package a `.app` only after it runs correctly.
- Do not download or install the Qwen model as part of this build. OptiQ and the model are already managed separately by the user.
- Do not run `cargo fetch` or any non-offline Cargo command during an audit. If the dependency cache is incomplete, report exactly which packages are missing and stop; don't silently start a large download.

## 5. Required sequence (one logical change per commit)

1. Fix the compile blocker in the HTTP SSE denial path; build/check without deleting features.
2. Capture the dependency tree and size baseline using `tools/local-qwen-offline/audit-light.sh`.
3. Remove cloud/account UI routes and disable their entry points; run build and smoke tests.
4. Remove server/auth/workflow dependencies only after their references are gone; rebuild.
5. Extract local Agent event types from the cloud MAA client/API schema; test shell/file/MCP action round trips.
6. Remove telemetry/crash/update transport and external AI/web providers; confirm no remote fallback.
7. Keep local indexing/MCP and prove them working.
8. Rebuild and compare `.app` size, executable size, `target/` size, number of unique crates, and number of network-capable code paths.
9. Generate/update third-party license inventory after dependency changes.

## 6. Acceptance checks before calling it stable

- [ ] `cargo build --offline -p warp --bin warp-oss` succeeds on the target Mac.
- [ ] App launches without login or email registration.
- [ ] Apple Foundation Models path is on-device only (no Private Cloud Compute fallback).
- [ ] OptiQ requests only go to loopback and work with the actual model ID returned by `/v1/models`.
- [ ] Local MCP server starts and tool results return to the local model.
- [ ] Local indexing/search finds files in a test repository without an external service.
- [ ] File edits, shell commands and computer-use actions remain permission-gated.
- [ ] With external network blocked at the OS/firewall level, the app still launches and local AI works.
- [ ] A packet/log audit shows no app-managed non-loopback requests during launch, idle, indexing, agent execution and shutdown.
- [ ] License inventory and notices still pass after removals.

## 7. Measurements not yet available

The current prototype has not produced a successful final `.app` in the reported install attempt. Therefore no honest before/after app-size reduction or final crate-removal count can be stated yet. The audit script records what can be measured from the local checkout without downloading dependencies.
