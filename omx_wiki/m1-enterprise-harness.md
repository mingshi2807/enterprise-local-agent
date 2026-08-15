---
title: "M1 Enterprise Harness"
tags: ["m1", "execution-harness", "audit", "budget", "cancellation", "policy"]
created: 2026-08-15T13:34:11.179Z
updated: 2026-08-15T13:34:11.179Z
sources: ["docs/proposal/proposal.md:608-1548", "docs/reports/reports_impl.md:135-312", "git tag m1-enterprise-harness"]
links: ["enterprise-local-agent-milestone-index.md", "m0-foundation.md", "m2-deterministic-loop.md"]
category: architecture
confidence: high
schemaVersion: 1
---

# M1 Enterprise Harness

## Status

Completed and committed as `3fc2215` / tag `m1-enterprise-harness`. Includes the terminal-budget and registry snapshot patch.

## Objective

Turn the M0 boundary into the mandatory enterprise execution membrane for lifecycle, policy, budgets, audit, cancellation, deadlines, model calls, and tool calls.

## Approved architecture and review decisions

- ExecutionHarness is the only governed path to ModelPort and ToolPort.
- RunContext stays single-owner and non-persistable; guarded operations use `&mut RunContext`.
- RunCancellationHandle is opaque and does not expose Tokio CancellationToken.
- Committed RunStatus is authoritative and cannot be reopened by audit failure.
- FailClosed RunStarted audit failure terminalizes the run and returns no usable cancellation handle.
- Cancellation terminalizes as Finished(Cancelled); deadline terminalizes as BudgetExceeded(Elapsed).
- Model order: preflight → reserve → ModelCallId → pre-audit → invoke → post-audit → sanitized result.
- Tool order: lookup → policy → reserve → audit → invoke → typed result. Policy denial consumes no ToolCalls.
- OperationEffect distinguishes NotInvoked, InvocationStarted, and StateCommitted.
- FailOpen marks audit degraded; FailClosed preserves whether invocation or state commitment occurred.
- Events remain metadata-only and schema version 2.

## Patch incorporated

Model/tool budget exhaustion now terminalizes the active run. Reserved slots are not refunded. ToolRegistry snapshots ToolPort::definition once and retains the same port binding.

## Implemented result

ExecutionHarness exposes start, model/tool invocation, complete, fail, and cancel operations. Deterministic fake ports and audit sinks are gated behind test-support.

## Verification evidence

Final historical patch verification: 41 tests passed; formatting, strict Clippy, dependency trees, deterministic CLI, and diff checks passed.

## Deliberate deferrals

Outer loop, Rig/providers, graphs, MCP, RAG, persistence, approvals, sandboxing, identity, remote idempotency, and parallel execution.

## Historical sources

- Proposal, approval, and patch specification: `docs/proposal/proposal.md`, lines 608–1548.
- Implementation and patch reports: `docs/reports/reports_impl.md`, lines 135–312.
- Index: [[enterprise-local-agent-milestone-index]].
- Previous: [[m0-foundation]]. Next: [[m2-deterministic-loop]].
