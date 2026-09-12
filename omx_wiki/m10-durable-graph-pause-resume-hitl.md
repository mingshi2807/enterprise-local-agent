---
title: "M10 Durable Graph Pause Resume and HITL"
tags: ["m10", "graph", "hitl", "durable-approval", "localwrite", "recovery"]
created: 2026-09-13
updated: 2026-09-13
sources: ["crates/agent-harness/src/durable_approval.rs", "crates/agent-harness/src/execution.rs", "crates/agent-harness/src/recovery.rs", "crates/agent-persistence-sqlite/src/lib.rs", "crates/agent-action-seal-local/src/", "crates/agent-graph/src/"]
links: ["enterprise-local-agent-milestone-index.md", "m6-approval-and-containment-boundary.md", "m6-1-production-linux-localwrite-containment.md", "m7-durable-event-persistence-and-recovery.md", "m9-deterministic-graph-engine-poc.md"]
category: architecture
confidence: high
schemaVersion: 1
---

# M10 Durable Graph Pause Resume and HITL

## Status

Implemented, verified, and Linux containment-certified in the working tree based
on commit `a1264ebed528c5b92861c9e96159d13ae5074a4d`. No M10 commit or tag has been
created.

## Scope and Authority

M10 adds intentional durable graph suspension for one trusted workflow mode:
human approval of the existing bounded `workspace_write_file` LocalWrite.
Immediate M6 `ApprovalPort` behavior is unchanged. Model output cannot select
durable approval mode.

```text
Graph DurableLocalWriteAction
  -> ExecutionHarness
  -> encrypted exact-action capsule
  -> durable Waiting
  -> trusted external decision
  -> explicit resume
  -> M6 policy/audit + M6.1 containment
```

`RecoveryDisposition::Waiting` is quiescent and distinct from both `Resumable`
and `ManualReconciliationRequired`. Waiting does not represent an unresolved
external effect.

## Waiting and Persistence

The versioned SQLite state machine is:

```text
Waiting
  -> DecisionRecordedApprove -> ApprovedReady -> Executing -> Consumed
  -> DecisionRecordedDeny -------------------------------> Consumed
```

The visibility order is Prepared, reserve one ApprovalRequests unit, REQUIRED
ApprovalRequested audit, then atomically publish the approval row together with
`GraphSuspended`. Prepared state is not externally approvable. Decision and
status changes use row-version CAS; duplicate, stale, swapped, wrong-run, or
wrong-binding submissions fail closed.

Store schema 2 adds `durable_approvals`. Event schema 9 and checkpoint schema 4
record only bounded correlation and graph-control metadata. Plaintext action
path/content is absent from events, checkpoints, audit, and SQLite rows.

## Exact Action Capsule

`LocalWriteActionCapsuleV1` contains only the exact typed relative path and
content required to resume `workspace_write_file`. It preserves the existing
240-byte relative-path and 4 KiB content limits.

The harness-owned `ActionSealPort` authenticates at least RunId, SessionId,
program version, graph digest, node and attempt, wait ID, ApprovalRequestId,
ActionProposalId, ToolCallId, ActionDigest, WorkspaceBindingId,
ToolContractDigest, and capsule version. `ToolContractDigest` is derived from
the trusted registered tool name, schema, description, and capability.

`agent-action-seal-local` implements sealing with XChaCha20-Poly1305. SQLite
stores only key ID, nonce, ciphertext, version, and bounded correlation
metadata. Deployment supplies key material outside SQLite. Nonces are random
and additionally constrained unique by the store. Payload-bearing types redact
Debug output; zeroization is used where practical without claiming complete
process-memory erasure.

This is the reviewed narrow exception to M7's no-payload persistence rule.
Encrypted capsules remain prohibited for unreviewed or arbitrary action types.

## Preview and Decision

An explicit approval-view operation loads the waiting record, validates its
durable bindings, decrypts and strictly decodes the capsule, revalidates the
trusted tool contract, and regenerates `ApprovalPreview`. The view contains
only operation, relative target, and content byte count. It never returns file
content. Recovery and replay do not decrypt capsules.

Recording Approve or Deny is persistence-only and never executes a tool.
Submissions must match the exact run, request, proposal, ToolCallId,
ActionDigest, and expected row version.

## Resume

Approve constructs a fresh process-local `RunContext`, verifies the recovery
contract and graph identity, then validates every capsule binding. It performs
exact tool lookup, schema/limit and capability checks, recomputes ActionDigest,
verifies ToolContractDigest and WorkspaceBindingId, recreates the internal
validated action, and rechecks policy, containment, and audit health.

Only then does the harness emit REQUIRED ApprovalGranted audit, atomically mark
Executing while reserving one ToolCall and durably recording
ToolInvocationStarted, emit REQUIRED tool-start audit, and dispatch through the
existing `ContainedToolPort`.

Deny emits the denial lifecycle, consumes the wait, charges no ToolCall, and
uses the graph's deterministic ApprovalDenied transition. Restart charges
neither another ApprovalRequest nor another GraphStep. The suspended Action
keeps its original node attempt, and resume never invokes the Action callback,
model, retrieval, or replanning.

## Crash Semantics

`ToolInvocationStarted` without a trustworthy terminal event remains
`ManualReconciliationRequired`; the tool is never retried automatically. A
crash after ToolInvocationCompleted but before graph continuation also never
repeats the write. If required transient ToolResult or application state is
unavailable, recovery is manual/non-resumable.

Audit and SQLite remain separate systems with no cross-system transaction
claim. Crash windows between them fail conservatively. Specialized durable
transition failures poison the process-local context and prevent dispatch.

## Dependencies

The new adapter uses `chacha20poly1305` 0.11.0 and `zeroize` 1.9.0. Existing
bundled `rusqlite` persistence remains single-process with rollback journal,
`synchronous=FULL`, bounded blocking work, and atomic event/checkpoint/wait CAS
transactions. No workflow framework, generic approval service, or distributed
database was added.

## Verification

Formatting, strict workspace Clippy, `git diff --check`, dependency and
Codebase Memory boundary scans passed. The workspace suite passed 248 tests and
four compile-fail doctests. Coverage includes restart while Waiting, durable
preview, approve/deny, exact ID preservation, duplicate and concurrent CAS,
tampering/wrong key/key ID, swapped bindings, tool/workspace/action digest
mismatches, capsule bounds/version, replay with zero decrypt/ports, one-time
budgets and callback, SQLite transaction fault injection, and both required
post-tool crash windows.

Explicit Linux certification passed without skips in
`enterprise-local-agent-m6-cert:2541e5c`. It used Bubblewrap 0.11.2 and a
root-owned mode-0555 static PIE musl worker with SHA-256
`c5606ddd01d7bb1584320976692fefe350bef74f0fd9fa35bf08b7de4621db53`.
Landlock reported `PartiallyEnforced`. The certification now includes the real
M5 -> M6 -> M6.1 -> M7 -> M9 -> M10 suspend, restart, approve, and contained
LocalWrite flow, plus process-tree reaping.

## Guarantees and Limits

M10 guarantees exact-action binding across intentional restart for this one
reviewed LocalWrite type, durable quiescent Waiting, CAS-protected decisions,
fresh runtime reconstruction, unchanged M6 governance, and real M6.1
containment on certified hosts.

It does not guarantee exactly-once external effects, atomicity between audit
and persistence, generic durable actions, durable arbitrary WorkingState,
complete memory erasure, automated key rotation, identity/RBAC, or successful
resume after the deployment key is unavailable. Native host Bubblewrap 0.6.1
remains unsupported.

## Forward Constraints

- Keep immediate ApprovalPort and durable approval as separate modes.
- Do not generalize encrypted capsules without a reviewed typed representation.
- Never expose plaintext capsules in events, checkpoints, audit, or errors.
- Never let UI/API/CLI/MCP become approval or execution authority.
- Never retry an uncertain or already completed write during recovery.
- Keep policy, required audit, budgets, and containment harness-owned.
