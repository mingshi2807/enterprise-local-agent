---
title: "Enterprise Local Agent Milestone Index"
tags: ["enterprise-local-agent", "milestones", "roadmap", "index"]
created: 2026-08-15T13:34:09.996Z
updated: 2026-09-21
sources: ["docs/proposal/proposal.md", "docs/reports/reports_impl.md", "git history"]
links: ["m0-foundation.md", "m1-enterprise-harness.md", "m2-deterministic-loop.md", "m3-rig-model-adapter.md", "m4-openai-compatible-gateway.md", "m5-typed-action-planning.md", "m6-approval-and-containment-boundary.md", "m6-1-production-linux-localwrite-containment.md", "m7-durable-event-persistence-and-recovery.md", "m8-enterprise-knowledge-integration.md", "m9-deterministic-graph-engine-poc.md", "m10-durable-graph-pause-resume-hitl.md", "m11-governed-mcp-integration.md", "m12-agent-service-api.md", "m13-local-enterprise-agent-mvp.md", "m14-deployment-operations-hardening.md", "m15-enterprise-identity-authorization.md", "m16-desktop-foundation.md"]
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
| M16 Desktop Foundation and ReadOnly Conversation | M16.0 approved; M16.1-M16.2 committed; M16.3 implemented and verified | Not tagged | [[m16-desktop-foundation]] |

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
desktop has no direct runtime or infrastructure authority.

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
