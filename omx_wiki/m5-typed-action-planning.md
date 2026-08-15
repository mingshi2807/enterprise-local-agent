---
title: "M5 Typed Action Planning"
tags: ["m5", "typed-actions", "json-schema", "action-proposal", "tool-policy"]
created: 2026-08-15T13:34:13.554Z
updated: 2026-08-15T13:34:13.554Z
sources: ["docs/proposal/proposal.md:4303-5291", "docs/reports/reports_impl.md:752-865", "git tag m5-typed-action-planning"]
links: ["enterprise-local-agent-milestone-index.md", "m4-openai-compatible-gateway.md", "m6-approval-and-containment-boundary.md"]
category: architecture
confidence: high
schemaVersion: 1
---

# M5 Typed Action Planning

## Status

Completed and committed as `f7be0e7` / tag `m5-typed-action-planning`.

## Objective

Allow Plan to decode an untrusted model response into a typed proposal while preserving ExecutionHarness as the sole authority for policy, budget, audit, and ToolPort invocation.

## Approved architecture and review decisions

- A proposal is not a ToolCall and never grants authorization.
- ActionProposalId is runtime-generated and distinct from the later ToolCallId.
- TextActionDecoder accepts one exact JSON envelope with action/tool/arguments and no prose, fences, trailing content, unknown envelope fields, normalization, or fuzzy matching.
- A recursive Serde visitor rejects duplicate keys during deserialization, including nested arguments.
- Argument property acceptance is governed by registered JSON Schema, not decoder knowledge.
- TextActionDecoder and ActionValidator remain separate so future native candidates can reuse validation.
- CompletedModelInvocation is opaque. ValidatedAction is single-use, non-cloneable, non-serializable, has no public constructor, and exposes no raw arguments.
- Use exactly jsonschema 0.49.2 with defaults disabled, compile once at registration, and force Draft 2020-12.
- The supported schema profile is intentionally narrow and rejects references, formats, patterns, composition/conditional keywords, type unions, and custom keywords.
- Fixed limits bound response, tool name, arguments, JSON complexity, and schema complexity.
- ActionExecutionBound means identity correlation only; policy and ToolCalls reservation occur afterwards.
- ToolResult call IDs are verified by the harness; mismatch is an adapter failure.
- ReadOnly executes; LocalWrite, ExternalWrite, and Privileged remain denied without consuming ToolCalls.
- Event schema version 4 records metadata-only proposal, validation, rejection, and binding events.
- The same deterministic ActionProgram is used with FakeRigModel and live OpenAI-compatible composition. Model text never chooses outer-loop transitions.

## Implemented result

Added provider-neutral action vocabulary, governed tracked-model/action APIs, strict decoding, compiled schema validation, ModelCallId → ActionProposalId → ToolCallId correlation, tool-result ID enforcement, and the deterministic ActionProgram.

## Verification evidence

Historical verification: 130 unit/integration tests plus four compile-fail doctests passed. Formatting, strict Clippy, dependency trees, boundary scans, diff checks, and the network-free CLI completed with Finished(Completed), usage 1/1/1, and audit_degraded=false.

## Deliberate deferrals

Native provider tool calls, Rig tool execution, structured output, approvals, write execution, retries, heuristic recovery, MCP, RAG, graphs, persistence, and multi-agent execution.

## Historical sources

- Proposal and approval: `docs/proposal/proposal.md`, lines 4303–5291.
- Implementation report: `docs/reports/reports_impl.md`, lines 752–865.
- Index: [[enterprise-local-agent-milestone-index]].
- Previous: [[m4-openai-compatible-gateway]]. Next: [[m6-approval-and-containment-boundary]].
