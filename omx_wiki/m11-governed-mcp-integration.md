---
title: "M11 Governed MCP Integration"
tags: ["m11", "mcp", "tools", "governance", "stdio", "recovery"]
created: 2026-09-13
updated: 2026-09-13
sources: ["crates/agent-mcp-adapters/src/lib.rs", "crates/agent-mcp-adapters/tests/stdio.rs", "crates/agent-harness/src/ports.rs", "crates/agent-harness/src/registry.rs", "crates/agent-harness/src/execution.rs"]
links: ["enterprise-local-agent-milestone-index.md", "m5-typed-action-planning.md", "m6-approval-and-containment-boundary.md", "m7-durable-event-persistence-and-recovery.md", "m8-enterprise-knowledge-integration.md", "m10-durable-graph-pause-resume-hitl.md", "m12-agent-service-api.md"]
category: architecture
confidence: high
schemaVersion: 1
---

# M11 Governed MCP Integration

## Status

Implemented, verified, and committed as
`14a673f3a2a8a9110c28b276d0f64cb01444aacd`. No M11 release tag has been
created.

## Scope and Authority

M11 adds governed executable MCP integration without making MCP an execution
authority:

```text
trusted MCP configuration
  -> pinned discovery and definition fingerprint
  -> ToolDefinition
  -> M5 ActionProposal / ValidatedAction
  -> ExecutionHarness
  -> policy / budget / audit / durable start
  -> ManagedToolPort
  -> allowlisted MCP tools/call
```

MCP types remain outside `agent-core`, `agent-loop`, `agent-graph`, and public
harness domain DTOs. Existing M8 knowledge retrieval remains separate through
`KnowledgePort`. M6 LocalWrite approval and M6.1 containment are unchanged.

## Protocol and Transport

M11 supports exactly MCP revision `2025-06-18`. Live initialization verified
that both configured enterprise servers accept it:

- standards-mcp `1.23.0` negotiated `2025-06-18`.
- rag-kag-ocpp `1.27.1` negotiated `2025-06-18`.

The preferred future revision `2026-07-28` was not supported by both installed
server SDKs and is therefore rejected. Initialization uses the revision's
`initialize` response followed by `notifications/initialized`; any negotiated
version mismatch fails closed.

Transport is bounded newline-delimited UTF-8 JSON-RPC over stdio. Trusted
configuration fixes the server ID, absolute executable, argv, explicit
environment, timeout, and discovery limits. There is no shell, HTTP transport,
retry, reconnect, or model-selected endpoint. The child starts with a cleared
environment and discarded stderr. The executable must be a regular executable,
must not be group/world writable, and must not carry setuid/setgid bits.

## Discovery and Registration

`ToolMapping` contains the exact allowlisted remote name, trusted local
`ToolName`, trusted description, trusted `CapabilityKind`, and expected
`DefinitionFingerprint`. Server descriptions and annotations do not select
runtime capability.

Discovery performs bounded `tools/list` pagination. It rejects missing tools,
duplicate names, conflicting definitions, repeated cursors, excessive pages,
excessive results, malformed JSON-RPC correlation, and oversized frames.

The definition fingerprint is SHA-256 over a deterministic canonical JSON
representation of the complete remote tool definition. A matching definition
is converted to the existing provider-neutral `ToolDefinition` and registered
as `ManagedDirect` through `ToolRegistry::register_managed`.

## Schema and Drift

The MCP input schema is not semantically rewritten. It must pass the existing
M5 bounded Draft 2020-12 schema profile. References, combinators, conditionals,
unsupported keywords, excessive size, and excessive complexity produce
`UnsupportedTool`, distinct from malformed MCP.

Each call uses a fresh MCP process and repeats protocol negotiation and
`tools/list`. The exact tool fingerprint must still match the registered
snapshot before `tools/call`. Invocation-time drift returns a sanitized adapter
failure and dispatches zero remote tool calls.

M11 rejects tools advertising `outputSchema`; it does not claim output-schema
validation. This keeps the first result profile narrow rather than accepting a
contract the runtime does not enforce.

## Capability and Result Mapping

Only trusted-configured ReadOnly tools may dispatch through MCP. Managed
LocalWrite cannot call MCP and reports containment unavailable. ExternalWrite
and Privileged remain denied and never dispatch.

Successful bounded text or structured JSON maps to `ToolResult::Succeeded`.
MCP `isError=true` maps to `ToolResult::DomainFailure`. Process, timeout,
protocol, framing, correlation, schema-drift, and malformed-response failures
map to sanitized infrastructure errors. Unsupported image, audio, resource, or
other rich content is rejected. Result content is never written to AgentEvent
or audit records.

Runtime `ToolCallId`, `ActionProposalId`, and `ActionDigest` remain independent
from JSON-RPC request IDs. MCP IDs are transport correlation only.

## Managed Lifecycle

The provider-neutral `ManagedToolPort` starts one `ManagedToolInvocation` only
after the harness reserves ToolCalls and durably records
`ToolInvocationStarted`. Cancellation and deadline paths call
`terminate_and_reap` before terminalizing the run. The stdio adapter owns a
fresh process group, sends SIGKILL to that group, and waits for the direct child.

This lifecycle is tested for the server and an ordinary descendant that remains
in the process group. It does not claim control over independently daemonized
or remote descendants.

## Recovery

M11 reuses M7's existing metadata-only tool lifecycle. Persistence failure for
the start transition prevents MCP process/tool dispatch. A durable
`ToolInvocationStarted` without a trustworthy terminal event recovers as
`ManualReconciliationRequired`; there is no automatic retry.

Replay has no access to MCP transport. It spawns no server, performs no
discovery, and sends no `tools/call`. M8 fresh-retrieval rules and M10's
LocalWrite-only encrypted capsules remain unchanged.

## Dependencies

No MCP SDK or agent framework was introduced. `agent-mcp-adapters` depends
inward on `agent-harness` and `agent-core`, and uses existing Tokio,
`serde_json`, SHA-256, `thiserror`, and `libc`. SQLite and `tempfile` are used
only by tests.

## Verification

Formatting, strict workspace Clippy, `git diff --check`, dependency scans, and
Codebase Memory blast-radius and boundary scans passed. The workspace suite
passed 265 tests and four compile-fail doctests. M11 contributes 15 adapter
tests plus two harness lifecycle tests.

Coverage includes pinned protocol success/mismatch, framing bounds, allowlist
matching, pagination and cursor loops, duplicate definitions, stable
fingerprints, invocation-time drift, M5 schema compatibility, trusted
capability mapping, success/domain/infrastructure result mapping, malformed
JSON-RPC IDs, timeout, process-group cleanup, durable-start failure with zero
dispatch, replay with zero MCP operations, and the full
Model -> M5 -> M6 policy -> MCP ReadOnly path.

The unchanged M6.1 Linux certification also passed in
`enterprise-local-agent-m6-cert:2541e5c` with Bubblewrap 0.11.2, a static musl
worker, process-tree reaping, and Landlock reported as `PartiallyEnforced`.

## Guarantees and Limits

M11 guarantees that configured MCP executable tools cannot bypass M5 action
validation, harness policy, budgets, audit, durable-start ordering, cancellation,
or runtime correlation. Definition drift is checked immediately before every
remote dispatch.

A configured MCP executable is trusted infrastructure. ReadOnly mapping limits
what the runtime requests; it does not prove the executable itself cannot
misbehave. M11 provides no OS sandbox for MCP servers, no exactly-once effects,
no HTTP transport, no rich content, no output-schema support, no retries, and no
LocalWrite, ExternalWrite, or Privileged MCP execution.

## Forward Constraints

- Keep MCP as transport/discovery rather than runtime authority.
- Never expose generic model-controlled `call_tool` or server selection.
- Keep capability mapping and allowlists in trusted configuration.
- Require invocation-time fingerprint verification before every call.
- Keep executable MCP LocalWrite unavailable until a reviewed contained path exists.
- Preserve durable-before-effect ordering and fail-closed recovery ambiguity.
- Sandbox untrusted MCP server executables in a future reviewed milestone.
