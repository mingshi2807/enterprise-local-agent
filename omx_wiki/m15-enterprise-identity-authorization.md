---
title: "M15 Enterprise Identity and Authorization"
tags: ["m15", "identity", "authentication", "authorization", "ownership", "approval"]
created: 2026-09-20
updated: 2026-09-20
sources: ["crates/agent-identity/src/lib.rs", "crates/agent-core/src/ids.rs", "crates/agent-service/src/lib.rs", "crates/agent-service-http/src/lib.rs", "crates/agent-persistence-sqlite/src/lib.rs", "crates/agent-harness/src/durable_approval.rs", "apps/agent-service-daemon/src/main.rs"]
links: ["enterprise-local-agent-milestone-index.md", "m14-deployment-operations-hardening.md", "m12-agent-service-api.md", "m10-durable-graph-pause-resume-hitl.md", "m6-approval-and-containment-boundary.md", "m6-1-production-linux-localwrite-containment.md"]
category: architecture
confidence: high
schemaVersion: 1
---

# M15 Enterprise Identity and Authorization

## Status

Implemented and verified in the current uncommitted worktree based on
`06ce65d8826d01630d7d4c169ec68e88946763b9`. No M15 tag has been created.

## Boundary

M15 answers who may request a service operation. It does not answer whether a
validated action may execute:

```text
transport authenticator
  -> VerifiedPrincipal
  -> agent-service authorization
  -> workflow/session/run operation
  -> existing ExecutionHarness authority
```

M6 `CapabilityPolicy`, M5 validation, approval, required audit, persistence,
budgets, recovery, and M6.1 containment remain independently authoritative.

## Principal and Authentication

`agent-identity` defines bounded `VerifiedPrincipal`, Human/Service/LocalProcess
principal kinds, User/Approver/Operator roles, and authentication class. Only
the bounded `PrincipalId` newtype enters `agent-core`; debug output is redacted.
Tokens, emails, claims blobs, credentials, PID identity, and arbitrary
attributes are excluded.

Authentication remains adapter-side. Unix sockets obtain UID/GID from real
Linux `SO_PEERCRED` and map only explicitly configured pairs. PID is not used as
durable identity and root gains no implicit role. Optional loopback bearer mode
maps one configured credential to exactly one configured service principal.
OIDC is deferred.

## Default-Deny Authorization

`ServiceAuthorizationPolicy` evaluates typed operations and resources. The
initial implementation is explicit default-deny and distinguishes workflow
start, owned run/status/events, cancellation, Waiting preview/decision/resume,
abort, and operational/reconciliation reads.

Owners control their sessions and runs. Approvers may access workflow-eligible
waits without owning the run. `RequesterMayApprove` and
`RequesterMustDiffer` are trusted configuration. Operators can inspect
operations and reconciliation metadata but gain no workflow start, approval,
resume, model, tool, policy, or execution authority.

## Durable Ownership and Approval Identity

Session ownership is checksum-protected in SQLite. Run creation atomically
persists immutable owner/requester, WorkflowId, authorization-policy version,
and policy fingerprint with the run record. Roles are not snapshotted as
permanent authority; every new command uses the current trusted policy.

Durable approval decisions persist actor PrincipalId, outcome, timestamp, and
CAS row version. M10 exact ActionProposalId, ToolCallId, ActionDigest, wait, and
request bindings remain unchanged. Requester identity is included in the
LocalWrite capsule AEAD authenticated data, so swapped requester records fail
authentication and revalidation.

## Legacy M14 State

No owner is inferred for pre-M15 records. Legacy unowned state is available
only for operator inspection. Unowned Waiting is projected as
`ManualReconciliationRequired` and cannot be normally approved or resumed.
No automatic or offline ownership migration utility is implemented.

## Audit and Recovery

Security mutations use required metadata-only phases:

```text
AuthorizationGranted
-> MutationRequested
-> required security audit
-> durable mutation
-> MutationCommitted | MutationFailed
```

A required audit failure before mutation causes zero mutation. Existing M6
LocalWrite audit remains separate. Only security-relevant records carry actor
identity; credentials and claim payloads never enter audit or AgentEvent.

Replay restores durable owner/requester/decision metadata but invokes no
authentication or authorization service. New post-restart commands authenticate
and authorize against current policy. Waiting resume still requires every M10
exact-action binding.

## Deployment Configuration

Identity configuration advances the active deployment schema to 2 and the Rust
type to `DeploymentConfigV2`. It contains bounded principal mappings, trusted
roles, approval separation, and authorization-policy version. The deterministic
deployment fingerprint includes security-relevant non-secret identity policy;
credential values remain externally referenced and excluded.

## Verification

The final worktree passed:

- `cargo fmt --all -- --check`;
- strict workspace Clippy with all targets and features;
- 310 workspace unit/integration tests and four compile-fail doctests;
- identity validation/redaction and default-deny policy tests;
- cross-principal isolation and delegated approval tests;
- RequesterMustDiffer, durable restart ownership, and legacy-unowned tests;
- requester-bound capsule tamper rejection and approval actor persistence;
- required security-audit ordering/failure and SQLite corruption/fault tests;
- Unix peer mapping, spoofed-header rejection, and credential leak scans;
- dependency/authority scans and Codebase Memory blast-radius analysis;
- explicit non-skipping M6.1 certification with Bubblewrap 0.11.2, a static
  root-owned mode-0555 worker, and Landlock `PartiallyEnforced`.

## Guarantees and Limits

M15 provides bounded authenticated principals, default-deny server-side
authorization, durable ownership, delegated approval identity, configurable
requester/approver separation, and fail-closed legacy handling without moving
execution authority.

It does not provide OIDC, SCIM, a general IAM policy language, automatic legacy
migration, distributed identity caching, multi-tenant billing, or implicit
operator execution authority. Authentication trust remains deployment-specific.

## Forward Constraints

- Keep authentication in transport/deployment adapters.
- Pass only verified principals into `agent-service`.
- Authorize from durable trusted ownership, never client IDs alone.
- Keep service authorization separate from M6 CapabilityPolicy.
- Preserve exact M10 action binding independently from approval identity.
- Never persist credentials, claims blobs, or unnecessary PII.
- Keep replay free of authentication, authorization, and external effects.
