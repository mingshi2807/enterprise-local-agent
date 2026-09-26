---
title: "Enterprise Local Agent Milestone Index"
tags: ["enterprise-local-agent", "milestones", "roadmap", "index"]
created: 2026-08-15T13:34:09.996Z
updated: 2026-09-26
sources: ["docs/proposal/proposal.md", "docs/reports/reports_impl.md", "git history"]
links: ["m0-foundation.md", "m1-enterprise-harness.md", "m2-deterministic-loop.md", "m3-rig-model-adapter.md", "m4-openai-compatible-gateway.md", "m5-typed-action-planning.md", "m6-approval-and-containment-boundary.md", "m6-1-production-linux-localwrite-containment.md", "m7-durable-event-persistence-and-recovery.md", "m8-enterprise-knowledge-integration.md", "m9-deterministic-graph-engine-poc.md", "m10-durable-graph-pause-resume-hitl.md", "m11-governed-mcp-integration.md", "m12-agent-service-api.md", "m13-local-enterprise-agent-mvp.md", "m14-deployment-operations-hardening.md", "m15-enterprise-identity-authorization.md", "m16-desktop-foundation.md", "m17-production-desktop-hardening.md"]
category: reference
confidence: high
schemaVersion: 1
---

# Enterprise Local Agent Milestone Index

This is the durable navigation page for architectural milestones. Git and the implementation remain authoritative; the wiki captures proposals, approved decisions, evidence, and forward constraints.

| Milestone | Status | Tag | Durable record |
| --- | --- | --- | --- |
| M0 Foundation | Completed | `m0-foundation` | [[m0-foundation]] |
| M1 Enterprise Harness | Completed | `m1-enterprise-harness` | [[m1-enterprise-harness]] |
| M2 Deterministic Loop | Completed | `m2-deterministic-loop` | [[m2-deterministic-loop]] |
| M3 Rig Model Adapter | Completed | `m3-rig-model-adapter` | [[m3-rig-model-adapter]] |
| M4 OpenAI-Compatible Gateway | Completed | `m4-openai-compatible-gateway` | [[m4-openai-compatible-gateway]] |
| M5 Typed Action Planning | Completed | `m5-typed-action-planning` | [[m5-typed-action-planning]] |
| M6 Approval and Containment Boundary | Completed | `m6-approval-containment` | [[m6-approval-and-containment-boundary]] |
| M6.1 Production Linux LocalWrite Containment | Completed and production-certified | `m6.1-linux-localwrite-containment` | [[m6-1-production-linux-localwrite-containment]] |
| M7 Durable Event Persistence and Recovery | Completed | `m7-durable-recovery` | [[m7-durable-event-persistence-and-recovery]] |
| M8 Enterprise Knowledge Integration | Completed | `m8-enterprise-knowledge` | [[m8-enterprise-knowledge-integration]] |
| M9 Deterministic Graph Engine PoC | Implemented and verified, uncommitted | Not tagged | [[m9-deterministic-graph-engine-poc]] |
| M10 Durable Graph Pause Resume and HITL | Implemented, verified, and Linux containment-certified, uncommitted | Not tagged | [[m10-durable-graph-pause-resume-hitl]] |
| M11 Governed MCP Integration | Implemented, verified, and committed | Not tagged | [[m11-governed-mcp-integration]] |
| M12 Agent Service API and Client Boundary | Implemented and verified, uncommitted | Not tagged | [[m12-agent-service-api]] |
| M13 Local Enterprise Agent MVP and Real LLM Smoke Test | Implemented, real-smoke verified, and Linux containment-certified, uncommitted | Not tagged | [[m13-local-enterprise-agent-mvp]] |
| M14 Deployment and Operations Hardening | Implemented, verified, Linux containment-certified, and committed | Not tagged | [[m14-deployment-operations-hardening]] |
| M15 Enterprise Identity and Authorization | Completed, verified, and Linux containment-certified | `m15-enterprise-identity` | [[m15-enterprise-identity-authorization]] |
| M16 Desktop Foundation and Governed Conversation | Completed and committed | `m16-desktop-app` | [[m16-desktop-foundation]] |
| M17 Production Desktop Hardening | Implemented, committed, and closed with RC validation items | Not tagged | [[m17-production-desktop-hardening]] |

## Stable architecture

Client -> Deterministic Loop or Graph Engine -> Enterprise Harness -> Model / Tools / Knowledge.

M10 adds trusted durable graph suspension for bounded LocalWrite approval. The
encrypted capsule adapter and SQLite wait storage remain behind harness-owned
ports; explicit resume returns through the existing M6/M6.1 execution path.

M11 adds pinned, allowlisted stdio MCP discovery and ReadOnly invocation behind
`ManagedToolPort`. MCP remains transport only: every executable call still
passes through M5 validation and harness-owned policy, budgets, audit, durable
start recording, cancellation, and terminal correlation.

M12 adds a provider-neutral service layer and bounded HTTP/JSON plus SSE
adapter. Clients observe and request operations while workflow selection,
recovery, policy, approval, audit, and execution authority remain server-side.
The default daemon listener is a restricted Unix socket; optional TCP is
loopback-only and authenticated.

M13 adds two reviewed enterprise workflows. ReadOnly performs real M8 retrieval
and grounded M4 model invocation without requiring containment. LocalWrite uses
the same fixed graph plus M5 validation, M10 durable Waiting, explicit resume,
and the existing M6/M6.1 governed write path. Real local-model smoke tests have
verified final answer, denied write, and approved restart/resume scenarios.

M14 adds strict versioned deployment configuration, independent workflow
readiness, metadata-only operational views and metrics, explicit draining,
offline SQLite backup and verified empty-target restore, fail-closed contract
compatibility, containment artifact integrity checks, and a non-authoritative
operator CLI. `agent-deployment` owns composition and operations only;
`ExecutionHarness` remains the execution and recovery authority.

M15 adds authenticated provider-neutral principals and default-deny service
authorization. Transport adapters authenticate; `agent-service` authorizes
each command against durable ownership and current trusted policy. Approval
identity records who decided without changing the M10 exact-action binding.
Legacy unowned records remain operator-only, and unowned Waiting is manual
reconciliation rather than resumable authority. Identity cannot bypass M5-M14
validation, policy, approval, audit, persistence, or containment.

M16 adds a thin Tauri 2 desktop client over the M12 service boundary. Its Rust
bridge owns Unix-socket or authenticated loopback connectivity and exposes only
named status and ReadOnly conversation commands; credentials and generic
transport access never enter the WebView. M16.3 runs only
`enterprise-engineering-readonly-v1`, consumes bounded `ServiceEventV2` cursor
pages, and renders the authoritative terminal answer with trusted citations.
Conversation state is volatile and in-memory, retries create new runs, and the
desktop has no direct runtime or infrastructure authority. M16.4 adds compact
safe activity, terminal summaries, and an optional metadata-only run inspector.
Its reducer accepts zero-based sequences, ignores duplicates, rejects ordering
gaps and wrong-run pages without cursor advancement, and never lets transient
activity override a terminal result. Markdown and inspector code are lazy-loaded
without changing the Tauri capability boundary. M16.5 adds the existing durable
LocalWrite approval path through named commands only: trusted Waiting preview,
CAS-bound Approve or Deny, explicit Resume, and abort. JavaScript never receives
capsules, action content, ActionDigest, ToolCallId, credentials, generic
transport, policy, or containment authority; all M10/M15/M6/M6.1 validation and
execution remain server-side.
M16.6 adds owner-authorized bounded session and per-session run projections.
The desktop restores its compact sidebar and selected-run metadata from the
service, catches up through existing event cursors, and reconstructs durable
Waiting without creating a duplicate run. SQLite remains adapter-side, no
conversation payload is stored in the WebView, and a missing volatile terminal
result is displayed as `Result unavailable` rather than regenerated.
M16.7 adds lazy-loaded observational Settings for local appearance and bounded
runtime, workflow, identity, and authorized operations status. It exposes no
deployment mutation or generic configuration authority. M16.8 completes the
desktop quality pass and native Tauri review across compact, normal, and wide
windows, light/dark themes, and 100%/150%/200% scaling. The review fixed
high-DPI clipping and status-projection defects without changing the named
command or capability boundary.
M16.9 adds credential-free unsigned `.app`/DMG and Debian packaging paths,
strict version/build metadata, deterministic compatibility manifests and
artifact hashes, package verification, and a documented external macOS signing
and notarization flow. The desktop still expects a separately installed trusted
service and bundles no model, knowledge backend, containment artifact, signing
credential, or automatic updater.

M17 adds a strict service/desktop compatibility handshake, fail-closed legacy
handling, generation-aware wake and reconnect recovery, adaptive bounded
polling, hostile-content and supply-chain hardening, lifecycle and history
stress evidence, and explicit platform release gates. The M11 process cleanup
investigation distinguishes live processes, zombies, disappearance, and PID
reuse; runtime cleanup remains unchanged and repeated tests prove that no live
owned descendant survives termination. Linux packaged and M6.1 gates pass;
native Wayland, physical suspend/wake, clean-source release manifest, and Apple
Silicon validation remain release-candidate items rather than claimed support.

Provider and framework types remain outside agent-core, agent-harness, agent-loop,
and agent-graph. ExecutionHarness remains the authority for lifecycle, budgets,
policy, audit, cancellation, deadlines, persistence, and external effects.

## Source-log policy

The historical append-only files remain available at `docs/proposal/proposal.md` and `docs/reports/reports_impl.md`. M6 and later should create or update milestone wiki dossiers instead of appending another complete transcript to those files.

## M6+ milestone dossier contract

Maintain exactly one compact wiki dossier for each milestone. Update the same page across the milestone lifecycle with:

1. Architecture proposal and scope boundaries.
2. Approval and review adjustments.
3. Implementation report and material deviations.
4. Verification evidence, including tests and quality gates.
5. Deliberate deferrals and forward constraints.

Keep the dossier curated rather than transcript-like. Git, committed source, and test results remain authoritative; the dossier is the durable navigation and decision record.
