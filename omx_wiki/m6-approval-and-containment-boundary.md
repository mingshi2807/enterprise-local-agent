---
title: "M6 Approval and Containment Boundary"
tags: ["m6", "approval", "containment", "sandbox-boundary", "local-write", "governance"]
created: 2026-08-15T14:24:55.310Z
updated: 2026-08-15T15:02:22Z
sources: ["commit 97ea322", "M6 architecture proposal conversation", "working tree M6 implementation"]
links: ["enterprise-local-agent-milestone-index.md", "m5-typed-action-planning.md"]
category: architecture
confidence: high
schemaVersion: 1
---

# M6 Approval and Containment Boundary

## Status

Implemented and verified in the working tree from committed M0-M5 baseline `97ea322`. Not committed.

## Objective

Extend governed typed actions so ReadOnly remains executable, LocalWrite requires explicit approval plus a contained executor, and ExternalWrite/Privileged remain non-executable. Authoritative path: `model -> ActionProposal -> ActionValidator -> ValidatedAction -> governed Act -> policy -> approval -> containment -> execution`. Policy, approval, and containment are independent; provider code remains model-only.

## Architecture proposal

### Approved design summary

Capability decision evolves to `Allowed | RequiresApproval | Denied(PolicyDenial)`. `M0ReadOnlyPolicy` stays unchanged. `M6ApprovalPolicy` maps ReadOnly to Allowed, LocalWrite to RequiresApproval, and ExternalWrite/Privileged to Denied. The harness also enforces an M6 safety ceiling: ReadOnly may execute directly or through a contained route; LocalWrite requires RequiresApproval plus a contained route; ExternalWrite and Privileged never execute even with a faulty custom policy.

Core owns shared metadata only: ApprovalRequestId, redacted ActionDigest, ApprovalRequests budget dimension, schema-v5 metadata events, and RunFailureKind::Approval/Containment. Harness owns orchestration objects: ApprovalRequest, ApprovalDecision, ApprovalOutcome, ApprovalPreview, ApprovalPort, ContainedToolPort, registry binding, strict auditing, and digest computation.

`ValidatedAction` remains single-use, non-cloneable, non-serializable, and inaccessible to mutation. It stores the registered capability and SHA-256 action digest. Approval decisions must match both request ID and digest. No enterprise actor identity is claimed in M6.

The digest uses a domain-separated versioned canonical binary encoding of exact tool name, trusted capability, and JSON arguments. Arrays retain order; object keys are sorted; values use explicit type and length encoding. It never authorizes execution or enters audit.

`ApprovalPreview` is non-serialized, redacted, trusted-adapter generated, and contains bounded 256-byte `summary`/`target_label` fields with no control characters. AgentEvent never carries preview text.

`ApprovalPort` is object-safe and receives only ApprovalRequest. ExecutionHarness owns cancellation/deadline while awaiting approval. Missing ports, port failures, decision mismatch, and denial fail closed.

Approval budget preserves `RunBudget::new(model_calls, tool_calls, iterations, elapsed)`. Approval defaults to zero and is enabled only with `with_max_approval_requests(n)`. Reservation occurs before ApprovalRequested audit and port invocation; exhaustion terminalizes as BudgetExceeded(ApprovalRequests); denial/failure consumes the approval slot but zero ToolCalls.

Schema v5 adds metadata-only ApprovalRequested, ApprovalGranted, ApprovalDenied, ApprovalFailed, and ContainmentFailed. LocalWrite uses required security auditing even under FailOpen. ApprovalGranted must persist before ToolCalls reservation, and ToolInvocationStarted before contained invocation.

`ContainedToolPort` is distinct from ToolPort, with no conversion from arbitrary ToolPort. ToolRegistry stores Direct versus Contained bindings but remains authorization-neutral. Implementing ContainedToolPort is a trusted adapter assertion, not OS isolation.

Governed LocalWrite sequence: preflight; consume ValidatedAction; bind ToolCallId; resolve binding; record ActionExecutionBound; evaluate policy and safety ceiling; require contained binding and non-degraded audit; generate preview; require ApprovalPort; reserve ApprovalRequests; record ApprovalRequested; await and verify decision; record denial/failure/grant; after recorded grant reserve ToolCalls; record ToolInvocationStarted; invoke contained executor once; verify ToolCallId; record terminal tool lifecycle.

Keep stable sanitized approval, containment, policy, budget, adapter, domain, cancellation, and deadline failures. `RunFailureKind::Approval`/`Containment` preserve loop terminal categories. Human denial fails the run before Verify/Reflect with no retry.

Cancellation wins deterministically over deadline and approval completion. A late approval after terminalization has no execution path.

## Dependency decision

Add exactly `sha2 = { version = "=0.11.0", default-features = false }` to agent-harness through workspace dependencies. Agent-core stores the digest without SHA-2. No other production dependency is added.

## Production containment boundary

M6 is contract-first: approval, digest binding, approval budget, strict audit, binding separation, and deterministic fakes. Production LocalWrite stays disabled until M6.1 supplies a real Linux containment adapter with verified write roots, no-network/environment controls, resource/output bounds, and kill-on-cancel behavior.

## Approval review notes

Approved adjustments: enforce a harness-owned M6 safety ceiling; preserve source compatibility of `RunBudget::new`; default approval budget to zero with a named builder; use deterministic serde_json::Number digest vectors; allow Direct/Contained ReadOnly without approval; require Contained LocalWrite for approval; centralize required security audit; persist ApprovalGranted before ToolCalls reservation and ToolInvocationStarted before contained invocation; use the normal tool lifecycle after ToolInvocationStarted; gate all fakes behind test-support/demo surfaces; label the CLI fake route as NO OS isolation; run full regression/boundary/CLI/dependency verification; do not run live external mode; do not commit.

## Implementation report

Implemented M6 in `agent-core`, `agent-harness`, `agent-loop`, `agent-cli`, README, and this wiki. Core additions are metadata-only: ApprovalRequestId, redacted ActionDigest, ApprovalRequests budget/usage, schema-v5 approval/containment events, and Approval/Containment run failure kinds. Harness additions are M6ApprovalPolicy, approval port/types, ContainedToolPort, Direct/Contained registry binding, SHA-256 digest computation, approval budget reservation, required-security-audit, and the M6 safety ceiling. Validated ReadOnly actions still obey CapabilityPolicy. Preview failures preserve sanitized containment categories. Cancellation or deadline after a contained invocation starts now records a terminal tool-invocation failure event while preserving the authoritative terminal RunStatus. Loop changes only map new harness errors into stable run failure kinds; six-phase control and LoopEffects are unchanged. CLI default remains ReadOnly; `--demo-local-write-fake-containment` uses FakeRigModel, scripted fake approval, fake contained execution, and prints fake/no-isolation labels.

## Verification evidence

Baseline before M6: 130 unit/integration tests and four compile-fail doctests passed from committed M0-M5.

M6 verification passed: `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets --all-features -- -D warnings`; `cargo test --workspace --all-features` with agent-cli 1, agent-core 25, agent-harness 85, agent-loop 23, agent-provider-rig 24, provider HTTP integration 5, and four doctests; `cargo tree --workspace`; `cargo tree -p agent-core`; `cargo tree -p agent-harness`; `cargo tree -p agent-loop`; `cargo tree -p agent-provider-rig`; `git diff --check`.

CLI evidence: default `cargo run -p agent-cli` completed with ModelCalls=1, ToolCalls=1, Iterations=1, ApprovalRequests=0, Finished(Completed). Explicit `cargo run -p agent-cli -- --demo-local-write-fake-containment` completed with ModelCalls=1, ToolCalls=1, Iterations=1, ApprovalRequests=1, Finished(Completed), and the exact `Approval: scripted fake` / `Containment: test fake — NO OS isolation` labels.

Boundary evidence: production scans found no Rig/OpenAI/provider/network imports in agent-core, agent-harness, or agent-loop; no approval/containment authority in agent-provider-rig; the provider crate's tool references remain test-only integration coverage; `sha2` appears only in workspace dependency metadata and agent-harness.

Deterministic coverage includes approval-budget defaults/exhaustion, Direct/Contained ReadOnly, policy-denied ReadOnly, Direct LocalWrite rejection, Contained LocalWrite grant/denial, missing/failing approval ports, preview failure categories, faulty-policy ceiling, degraded-audit blocking, required-audit failures, pending approval and contained-execution cancellation/deadline, terminal tool lifecycle correlation, decision ID/digest mismatch, ToolResult call-id mismatch, metadata-only events, and fixed digest vectors. An independent code review found and then verified resolution of the contained cancellation/deadline lifecycle gap; its final verdict was approve with no remaining findings.

## Deliberate deferrals

Real Linux sandboxing, arbitrary filesystem mutation, ExternalWrite, Privileged execution, enterprise identity/SSO/RBAC, separation of duties, durable HITL, persistence, background approval queues, interactive approval, Rig/native provider tool execution, MCP, RAG, graphs, multi-agent execution, retries, streaming, structured output, native provider tool-call correlation, and rollback semantics after a started write.

## Risks and tradeoffs

The containment trait is trusted rather than machine-verifiable; approval is in-memory and proves no actor identity. Strict audit may make LocalWrite unavailable. Digest encoding is security-sensitive and versioned/tested. Post-execution audit cannot roll back a write.

## Navigation

- Index: [[enterprise-local-agent-milestone-index]].
- Previous: [[m5-typed-action-planning]].
