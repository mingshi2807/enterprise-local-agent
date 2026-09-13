---
title: "M12 Agent Service API and Client Boundary"
tags: ["m12", "service", "http", "sse", "hitl", "recovery"]
created: 2026-09-13
updated: 2026-09-13
sources: ["crates/agent-service/src/lib.rs", "crates/agent-service-http/src/lib.rs", "apps/agent-service-daemon/src/main.rs", "crates/agent-harness/src/read.rs", "crates/agent-persistence-sqlite/src/lib.rs"]
links: ["enterprise-local-agent-milestone-index.md", "m7-durable-event-persistence-and-recovery.md", "m10-durable-graph-pause-resume-hitl.md", "m11-governed-mcp-integration.md"]
category: architecture
confidence: high
schemaVersion: 1
---

# M12 Agent Service API and Client Boundary

## Status

Implemented and verified in the current uncommitted worktree based on
`27bba306edb6bf800eb336e34906fa63e1d9c5ee`. No M12 tag or commit has been
created.

## Scope and Authority

M12 exposes the governed runtime to local clients without transferring
execution authority:

```text
CLI / Tauri / Web / IDE
  -> agent-service-http
  -> agent-service
  -> trusted graph or loop workflow
  -> ExecutionHarness
```

Clients can create sessions, start configured workflows, inspect status, read
or stream metadata events, manage durable Waiting approvals, explicitly resume
runs, cancel active runs, and abort Waiting runs. They never receive direct
access to model, knowledge, tool, approval, MCP, persistence, policy, audit, or
containment ports.

`WorkflowId` is resolved only through the trusted server-side catalog. Request
payloads cannot select providers, tools, MCP servers, endpoints, policies,
approval implementations, or containment configuration.

## Service API

`agent-service` defines provider-neutral request/view types and these
application operations:

- `create_session`
- `start_run`
- `get_run_status`
- `read_events`
- `waiting_page`
- `approval_preview`
- `submit_decision`
- `resume_run`
- `cancel_active_run`
- `abort_waiting`
- `discover_runs`

Run input is non-empty and at most 8 KiB. Payload-bearing input has redacted
`Debug`. Errors are converted to stable categories without exposing internal
configuration, persistence details, or payloads.

## Passive Read Boundary

The harness-owned `RunReadPort` separates passive queries from execution and
mutation authority. It returns bounded provider-neutral run summaries, event
pages, and Waiting summaries. SQLite implements it with the existing bounded
`spawn_blocking` semaphore.

Event reads validate store identity, schema versions, checksums, run/session
identity, exact sequence continuity, and cursor position before projection.
Waiting and run pagination use deterministic typed cursors. SQLite types and
connections do not enter service APIs.

## Ownership and Idempotency

`AgentService` is the `RunSupervisor`. It permits one active run per session
and at most 32 active runs globally. Capacity is reserved before spawning, and
no supervisor lock is held across an await point.

`start_request_id` and `SessionId` deterministically derive `RunId`. Repeating
the same request returns the same active or durable run and cannot invoke the
workflow twice. A different start while the session is active is rejected.
Resume uses the same ownership reservation and rejects duplicate concurrent
resume.

Cancellation remains harness-authoritative through `RunCancellationHandle`.
A transport disconnect does not request cancellation.

## HTTP and Daemon Security

`agent-service-http` provides bounded HTTP/JSON commands and SSE. Request
bodies are capped at 16 KiB. Mutating requests use a fail-fast 32-command
semaphore instead of an unbounded waiter queue.

The daemon uses a mode-0600 Unix socket by default. Its data directory must be
an absolute directory owned by the effective user and is restricted to mode
0700. A non-blocking exclusive lock prevents a second mutable daemon owner.
Only an owned stale socket may be removed; other filesystem objects fail
closed.

Optional TCP accepts only loopback addresses and requires a bearer token of at
least 32 bytes plus exact non-wildcard Host and Origin allowlists. There is no
non-loopback listener, wildcard CORS, HTTP MCP management, or remote plugin
installation.

## Events and SSE

`ServiceEventV1` is an explicit stable metadata-only projection. It contains
only version, sequence, run ID, category, phase, and an optional runtime
correlation ID. It never serializes internal `AgentEvent` directly.

No prompt, query, evidence, action argument, `ToolResult`, approval preview,
capsule, ciphertext, audit internals, credential, or configuration payload is
exposed. `EventSequence` is the SSE ID and reconnect cursor.

SSE repeatedly reads verified durable pages, so events committed between
catch-up and live polling are not lost. Pages are capped at 64 records, streams
at 64 globally and four per run, and lifetime at five minutes. Saturated or
slow clients disconnect and reconnect from their durable cursor rather than
blocking runtime execution. Invalid future cursors produce an explicit replay
boundary error.

## Durable HITL and Abort

Approval decision requests contain only the wait ID, expected row version, and
Approve or Deny. The service does not ask clients to echo `ApprovalRequestId`,
`ActionProposalId`, `ToolCallId`, `ActionDigest`, or capsule bindings. The
harness loads and verifies those values from the durable M10 record.

Preview delegates to the existing M10 view operation and returns only the
trusted operation summary, relative target, byte count, wait ID, and row
version. Content is never returned.

Approve/Deny recording is CAS-bound and does not execute. Explicit resume is a
separate command and returns through M10 restore validation and M6/M6.1
governance. Aborting Waiting atomically persists terminal Cancelled status and
consumes the wait, with zero ToolCalls. A later stale approval fails closed.
Abort is distinct from a human Deny decision.

## Startup and Recovery

The daemon first acquires exclusive data-directory ownership, builds trusted
composition, opens persistence and audit, and then discovers durable runs.
Discovery invokes no external ports and performs no capsule decryption.

Each run is mapped to its exact trusted `RecoveryContract` and exposed as
Completed, Failed, Waiting, Resumable, or ManualReconciliationRequired. There
is no automatic retry of unresolved effects. Explicit resume constructs a
fresh process-local context through the existing harness recovery contract.

## Dependencies

`agent-service` depends inward on harness plus the graph/loop orchestration
surface. `agent-service-http` adds Axum only at the transport boundary. The
daemon composes service, HTTP, harness, SQLite persistence, metadata audit, and
trusted workflows. `rustix` supplies advisory locking and ownership checks.
No web types enter core, harness domain APIs, loop, or graph.

## Verification

Current-artifact results:

- `cargo fmt --all -- --check`: passed.
- strict workspace Clippy with all targets/features: passed.
- workspace tests with all features: 276 unit/integration tests plus four
  compile-fail doctests passed.
- SQLite transaction-fault and corruption tests: passed.
- dependency and forbidden-import scans: passed.
- Codebase Memory index and blast-radius scan: passed with the expected
  service, harness, persistence, graph-test, and containment-regression impact.
- Unix-socket daemon request smoke: passed.

The unchanged M6.1 certification was rerun without skips in
`enterprise-local-agent-m6-cert:2541e5c`. Bubblewrap 0.11.2 and a root-owned
mode-0555 static musl worker passed the complete certification suite, including
process-tree reaping. Landlock reported `PartiallyEnforced`. The worker SHA-256
for this worktree was
`fb5d6066384f65b0934b46dd05fbc2f84566d0c9f580b63c312a89a35e7d2295`.

M12-specific tests cover idempotent/concurrent start, status, cancellation,
metadata-only ordered events, restart discovery, loopback authentication,
Host/Origin rejection, oversized bodies, bounded command concurrency, SSE
cursor reconnect and connection saturation, exclusive directory ownership,
and safe stale-socket handling. Existing M10 tests cover durable preview,
approve, deny, resume, abort, stale decisions, zero duplicate execution, and
replay inertness.

## Guarantees and Limits

M12 guarantees that clients remain request/observation actors and cannot bypass
`ExecutionHarness`. It provides bounded local transport, deterministic start
idempotency, single-owner active runs, metadata-only streaming, conservative
recovery exposure, and CAS-bound HITL commands.

The initial daemon registers only the fixed `service-health` workflow. Reviewed
production graph/loop/model/knowledge/tool compositions are not yet wired into
the daemon. The complete HTTP Waiting -> Approve -> resume -> real contained
LocalWrite flow has not yet received a dedicated transport-level integration
test; its service, M10, and M6.1 layers are tested separately.

There is no ACP adapter, Tauri client, Web UI, RBAC/SSO, non-loopback service,
distributed worker, multi-process coordination, exactly-once effect guarantee,
or audit/SQLite atomicity claim. Graceful signal cleanup is implemented, while
owned stale-socket recovery is the behavior directly verified in this runner.

## Forward Constraints

- Keep all execution and recovery authority in `ExecutionHarness`.
- Keep client and transport DTOs separate from core domain types.
- Add workflows only through reviewed trusted server-side composition.
- Add a full HTTP durable-approval/contained-LocalWrite certification test
  before claiming transport-level HITL certification.
- Keep event projections metadata-only and versioned.
- Preserve deterministic start IDs, CAS decisions, and single-run ownership.
- Future ACP, IDE, Tauri, or Web adapters must call `agent-service`; UI code
  must never own runtime ports or persistence.
