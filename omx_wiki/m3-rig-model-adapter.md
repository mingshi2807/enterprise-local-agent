---
title: "M3 Rig Model Adapter"
tags: ["m3", "rig", "model-adapter", "completion-model", "provider-boundary"]
created: 2026-08-15T13:34:12.370Z
updated: 2026-08-15T13:34:12.370Z
sources: ["docs/proposal/proposal.md:2505-3347", "docs/reports/reports_impl.md:407-562", "git tag m3-rig-model-adapter"]
links: ["enterprise-local-agent-milestone-index.md", "m2-deterministic-loop.md", "m4-openai-compatible-gateway.md"]
category: architecture
confidence: high
schemaVersion: 1
---

# M3 Rig Model Adapter

## Status

Completed and committed as `61d6095` / tag `m3-rig-model-adapter`.

## Objective

Integrate Rig as a replaceable ModelPort adapter without allowing Rig to become the runtime architecture.

## Approved architecture and review decisions

- Use exactly `rig-core = =0.41.0` with default features disabled; no rig facade or rig-agent.
- RigModelAdapter<M> is generic over CompletionModel; type erasure remains Arc<dyn ModelPort>.
- Core, harness, and loop expose no Rig types or dependencies.
- CompletionRequest is constructed field-by-field as an upgrade tripwire; tools, schemas, telemetry content, and provider overrides remain disabled.
- System/User/Assistant order is preserved explicitly.
- Only text output is accepted. Tool calls, reasoning, images, and mixed content fail atomically.
- Usage maps only nonzero input_tokens and output_tokens.
- Completion errors map to stable sanitized ModelPortError categories; provider strings and bodies never cross the boundary.
- One ModelPort invocation performs one completion await with no adapter timeout, retry, or cancellation domain.
- A deterministic FakeRigModel implements the actual pinned CompletionModel contract.

## Implemented result

Added agent-provider-rig. The runtime path is LoopEngine → LoopEffects → ExecutionHarness → ModelPort → RigModelAdapter → CompletionModel. Existing ReadOnly tools remain exclusively on ToolPort.

## Verification evidence

Historical verification: 93 unit tests plus one doctest passed; 17 adapter tests, strict Clippy, formatting, trees, boundary scans, deterministic CLI, and independent review all passed.

## Deliberate deferrals

Real endpoints/providers, native tool calls, streaming, structured output, retries/idempotency, RAG, MCP, persistence, graph execution, and remote cancellation guarantees.

## Historical sources

- Proposal and approval: `docs/proposal/proposal.md`, lines 2505–3347.
- Implementation report: `docs/reports/reports_impl.md`, lines 407–562.
- Index: [[enterprise-local-agent-milestone-index]].
- Previous: [[m2-deterministic-loop]]. Next: [[m4-openai-compatible-gateway]].
