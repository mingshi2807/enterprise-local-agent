---
title: "M2 Deterministic Loop"
tags: ["m2", "deterministic-loop", "state-machine", "iterations", "loop-engine"]
created: 2026-08-15T13:34:11.775Z
updated: 2026-08-15T13:34:11.775Z
sources: ["docs/proposal/proposal.md:1549-2504", "docs/reports/reports_impl.md:313-406", "git tag m2-deterministic-loop"]
links: ["enterprise-local-agent-milestone-index.md", "m1-enterprise-harness.md", "m3-rig-model-adapter.md"]
category: architecture
confidence: high
schemaVersion: 1
---

# M2 Deterministic Loop

## Status

Completed and committed as `ecefd5a` / tag `m2-deterministic-loop`.

## Objective

Implement the typed sequential outer loop Observe → Retrieve → Plan → Act → Verify → Reflect under Rust control.

## Approved architecture and review decisions

- LoopPhase is shared metadata in agent-core; LoopState, VerificationResult, and ReflectDecision remain orchestration-local.
- LoopState is a pure synchronous state machine and has no M2 serialization contract.
- Exactly one LoopProgram trait provides six typed phase methods and a program-owned WorkingState.
- LoopEffects exposes only harness-forwarding capabilities; LoopProgram is trusted in-process code, not a sandbox.
- ExecutionHarness::checkpoint owns between-phase cancellation/deadline detection.
- RunContext status is checked after every phase future; a terminal context cannot advance.
- ExecutionHarness::begin_iteration reserves a permanent one-based iteration before Observe.
- Zero/exhausted iteration budget prevents all phase execution and terminalizes as BudgetExceeded(Iterations).
- Illegal transitions terminalize active runs as invariant failures rather than panicking.
- Reflect alone normally chooses Continue, Complete, or Fail; free-form model output cannot jump phases.
- Loop events are metadata-only, use EventSequence ordering, and advance the event schema to version 3.
- Execution remains sequential with `&mut RunContext`.

## Implemented result

Added agent-loop with LoopEngine, LoopState, LoopProgram, LoopEffects, typed transitions, summaries, and stable error handling. Caller starts the run; LoopEngine owns normal completion/failure finalization.

## Verification evidence

Historical verification: 76 tests plus the LoopState compile-fail doctest passed. Formatting, strict Clippy, dependency trees, payload scans, and deterministic 1/1/1 CLI smoke passed.

## Deliberate deferrals

Providers, real retrieval/RAG, MCP, graphs, checkpoints, approvals, sandboxing, identity, retries, fan-out/fan-in, and multi-agent execution.

## Historical sources

- Proposal and approval: `docs/proposal/proposal.md`, lines 1549–2504.
- Implementation report: `docs/reports/reports_impl.md`, lines 313–406.
- Index: [[enterprise-local-agent-milestone-index]].
- Previous: [[m1-enterprise-harness]]. Next: [[m3-rig-model-adapter]].
