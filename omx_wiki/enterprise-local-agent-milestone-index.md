---
title: "Enterprise Local Agent Milestone Index"
tags: ["enterprise-local-agent", "milestones", "roadmap", "index"]
created: 2026-08-15T13:34:09.996Z
updated: 2026-09-13
sources: ["docs/proposal/proposal.md", "docs/reports/reports_impl.md", "git history"]
links: ["m0-foundation.md", "m1-enterprise-harness.md", "m2-deterministic-loop.md", "m3-rig-model-adapter.md", "m4-openai-compatible-gateway.md", "m5-typed-action-planning.md", "m6-approval-and-containment-boundary.md", "m6-1-production-linux-localwrite-containment.md", "m7-durable-event-persistence-and-recovery.md", "m8-enterprise-knowledge-integration.md", "m9-deterministic-graph-engine-poc.md", "m10-durable-graph-pause-resume-hitl.md"]
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

## Stable architecture

Client -> Deterministic Loop or Graph Engine -> Enterprise Harness -> Model / Tools / Knowledge.

M10 adds trusted durable graph suspension for bounded LocalWrite approval. The
encrypted capsule adapter and SQLite wait storage remain behind harness-owned
ports; explicit resume returns through the existing M6/M6.1 execution path.

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
