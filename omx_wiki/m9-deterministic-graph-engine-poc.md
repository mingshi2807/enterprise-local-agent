---
title: "M9 Deterministic Graph Engine PoC"
tags: ["m9", "graph", "deterministic", "orchestration", "recovery"]
created: 2026-09-06
updated: 2026-09-06
sources: ["crates/agent-graph/src/", "crates/agent-core/src/graph_control.rs", "crates/agent-harness/src/execution.rs", "crates/agent-harness/src/recovery.rs"]
links: ["enterprise-local-agent-milestone-index.md", "m7-durable-event-persistence-and-recovery.md", "m8-enterprise-knowledge-integration.md", "m6-1-production-linux-localwrite-containment.md"]
category: architecture
confidence: high
schemaVersion: 1
---

# M9 Deterministic Graph Engine PoC

## Status

Implemented and verified in the working tree. No M9 commit or tag has been
created.

## Scope and Architecture

`agent-graph` is a provider-neutral sibling of `agent-loop`. It owns bounded
workflow orchestration only:

```text
agent-graph -> agent-harness -> agent-core
                         +-> agent-knowledge
```

Graph callbacks receive only `RetrieveEffects`, `ModelEffects`,
`ActionEffects`, `VerifyEffects`, or `DecisionContext`. The first three
delegate to `ExecutionHarness`; Verify and Decision expose no external effect.
Graph code has no direct port, registry, policy, provider, Rig, or MCP access.

## Domain, Validation, and Digest

`GraphDefinition` is immutable after construction and stores bounded IDs,
nodes, and typed transitions. Supported node kinds are Retrieve, Model, Action,
Verify, Decision, Complete, and Fail. Construction rejects unknown or duplicate
nodes/transitions, invalid transition sets, terminal outgoing edges,
unreachable nodes, every cycle, invalid recovery modes, and graphs exceeding
64 nodes or 128 edges.

The SHA-256 definition digest uses an explicit domain separator, explicit
stable kind/recovery/transition tags, length-prefixed IDs, and sorted maps and
decision branches. It is independent of input vector and map iteration order.

## Execution and Budget

For each node, `GraphEngine` asks the harness to preflight and durably reserve a
`GraphSteps` unit before invoking the typed callback. The callback returns one
typed outcome, which resolves exactly one prevalidated transition. Completion
and next-node metadata are persisted before the next node. Callback failure or
an unavailable transition terminalizes the run; there is no retry or fallback
edge. Complete and Fail nodes durably finalize the run.

`RunBudget::new` remains unchanged. Graph execution is disabled by default and
requires `.with_max_graph_steps(n)`. The limit is independent of loop
iterations and cannot exceed 64.

Graph routing is deterministic from a typed outcome. `GraphProgram` callbacks
are trusted in-process application code and are not claimed pure or
deterministic.

## PoC Workflow and Authority

The tested PoC is:

```text
Retrieve -> grounded Model -> Action -> Verify
                                      | passed -> Complete
                                      | failed -> Fail
```

Retrieve uses M8 `KnowledgePort` through the harness. Model uses the existing
grounded model path. Action uses M5 action preparation and M6 policy, approval,
required audit, and containment. A graph test drives LocalWrite through a
`ContainedToolPort` fake and proves approval, containment preview, execution,
budget, and `ToolCallId` correlation remain harness-owned.

## Persistence and Recovery

Event schema 8 adds metadata-only GraphStarted, GraphNodeEntered,
GraphNodeRestarted, GraphNodeCompleted, GraphCompleted, and GraphFailed events.
Checkpoint schema 3 adds only graph digest, bounded node identity/kind,
attempt ID, step count, recovery mode, and fresh-retrieval anchor. It persists
no evidence, prompts, model responses, action arguments, `ValidatedAction`,
tool results, or arbitrary working state.

The existing harness reducer processes graph events during live execution and
replay. Replay has no callback or external-port access. A matching
`RestartableGraphProgram` reconstructs only explicitly supported
provider-neutral state in a fresh `RunContext`.

Retrieve recovery is `FreshRetrieval` only. Lost or interrupted evidence causes
a new retrieval from the durable anchor, never reconstruction of historical
evidence. Decision may resume only at a `DeterministicBoundary` reconstructable
from durable metadata. Model, Action, Verify, Complete, and Fail use `Never`.
Unresolved model, approval, tool, or containment effects retain M7
`ManualReconciliationRequired` handling. Program-version or definition-digest
mismatch fails closed.

## Dependencies

Production `agent-graph` depends only on workspace `agent-core`,
`agent-harness`, `agent-knowledge`, `sha2`, and `thiserror`. SQLite, Tokio test
runtime, JSON fixtures, and temporary directories are dev-only dependencies.
No graph framework, provider SDK, MCP client, HTTP client, or `agent-loop`
dependency is introduced.

## Guarantees and Limits

M9 guarantees validation of the bounded DAG, deterministic typed routing,
harness-only effects, a separate hard graph-step budget, metadata-only durable
control state, inert replay, and conservative restart classification. It does
not guarantee callback purity, exactly-once effects, arbitrary working-state
reconstruction, historical evidence availability, dynamic graph mutation,
parallel execution, arbitrary cycles, or automatic retries.

## Verification

Final formatting, strict workspace Clippy, dependency trees, boundary scans,
Codebase Memory reindex/change detection, and `git diff --check` passed. The
workspace suite passed 241 tests plus four compile-fail doctests. This includes
16 graph tests, 105 harness tests, the SQLite injected-transaction
all-or-nothing test, unchanged LoopEngine regressions, inert replay, fresh
Retrieve restart, recovery mismatch handling, and governed LocalWrite through
approval and contained execution.

The explicit M6.1 Linux certification passed without skips in the retained
production-capable image using Bubblewrap 0.11.2 and an immutable mode-0555
static PIE musl worker. Optional Landlock reported `PartiallyEnforced`. The
certification includes containment capability probing, create/replace, escape
and network controls, environment/FD isolation, process-tree reaping,
ToolCallId correlation, and LocalWrite recovery regression.

## Forward Constraints

- Keep graph orchestration separate from application/business state.
- Do not expose ports, registries, policy, approval, or containment directly to
  graph nodes.
- Do not persist payloads to make additional nodes restartable.
- Do not reinterpret unresolved external effects as safe to retry.
- Keep `LoopEngine` unchanged as the fixed-loop sibling.
