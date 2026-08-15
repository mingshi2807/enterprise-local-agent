---
title: "M0 Foundation"
tags: ["m0", "foundation", "agent-core", "agent-harness", "provider-neutral"]
created: 2026-08-15T13:34:10.588Z
updated: 2026-08-15T13:34:10.588Z
sources: ["docs/proposal/proposal.md:3-607", "docs/reports/reports_impl.md:3-134", "git tag m0-foundation"]
links: ["enterprise-local-agent-milestone-index.md", "m1-enterprise-harness.md"]
category: architecture
confidence: high
schemaVersion: 1
---

# M0 Foundation

## Status

Completed and committed as `16e16dd` / tag `m0-foundation`.

## Objective

Establish the provider-independent Rust workspace and domain contracts without performing real model or tool execution.

## Approved architecture and review decisions

- Dependency direction: agent-cli → agent-harness → agent-core, with optional direct CLI → core.
- RunContext and the object-safe ModelPort, ToolPort, and AuditSink belong in agent-harness.
- ToolRegistry owns registration, exact lookup, uniqueness, and metadata integrity; CapabilityPolicy owns authorization.
- M0ReadOnlyPolicy allows ReadOnly and denies LocalWrite, ExternalWrite, and Privileged.
- RunBudget defines model calls, tool calls, and elapsed time; iteration semantics were deliberately deferred.
- RunStatus has one terminal representation: Finished(RunOutcome).
- PortFuture uses boxed Send futures without async-trait or Tokio.
- Payload-bearing Debug output is absent or redacted. AgentEvent is schema-versioned, metadata-only, and ordered by EventSequence.
- UserId, providers, runtime deadlines, and persistence were deferred.

## Implemented result

Created agent-core, agent-harness, and agent-cli. Core owns typed IDs, budgets, run state, capabilities, model/tool contracts, and events. Harness owns RunContext, registry, policy, and the three port traits.

## Verification evidence

Historical milestone verification: 22 tests passed; formatting, strict Clippy, dependency-tree checks, CLI smoke, and read-only review passed.

## Deliberate deferrals

Rig, Tokio, real invocation, graph execution, MCP, RAG, persistence, identity, approvals, sandboxing, networking, and provider integrations.

## Historical sources

- Proposal and approval: `docs/proposal/proposal.md`, lines 3–607.
- Implementation report: `docs/reports/reports_impl.md`, lines 3–134.
- Index: [[enterprise-local-agent-milestone-index]].
- Next milestone: [[m1-enterprise-harness]].
