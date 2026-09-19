---
title: "M13 Local Enterprise Agent MVP and Real LLM Smoke Test"
tags: ["m13", "mvp", "local-llm", "knowledge", "localwrite", "smoke-test"]
created: 2026-09-20
updated: 2026-09-20
sources: ["crates/agent-mvp/src/lib.rs", "crates/agent-mvp/tests/real_local_model.rs", "apps/agent-service-daemon/src/main.rs", "crates/agent-provider-rig/src/openai_compatible.rs", "crates/agent-knowledge/src/lib.rs"]
links: ["enterprise-local-agent-milestone-index.md", "m8-enterprise-knowledge-integration.md", "m10-durable-graph-pause-resume-hitl.md", "m12-agent-service-api.md", "m6-1-production-linux-localwrite-containment.md"]
category: architecture
confidence: high
schemaVersion: 1
---

# M13 Local Enterprise Agent MVP and Real LLM Smoke Test

## Status

Implemented and verified in the current uncommitted worktree based on
`803fe05797ea325bae6b138e875163c8c4f78822`. All three explicitly opted-in real
smoke scenarios passed. No M13 tag or commit has been created.

## Application Composition

M13 adds `agent-mvp` and replaces the daemon's health-only composition with two
reviewed immutable workflows while retaining `service-health`:

```text
client
  -> agent-service
  -> reviewed M13 graph
  -> ExecutionHarness
  -> KnowledgePort / ModelPort / governed LocalWrite
```

Runtime authority remains in `ExecutionHarness`. Workflow selection resolves
through the trusted server-side catalog. Clients and model output cannot select
model configuration, knowledge endpoints, MCP servers, tools, policy, approval,
audit, action sealing, workspace binding, or containment.

## Reviewed Workflows

`enterprise-engineering-readonly-v1` executes:

```text
Retrieve -> Model -> VerifyAnswer -> Complete
```

It returns a bounded final answer with citations and has no M6.1 dependency.

`enterprise-engineering-localwrite-v1` executes:

```text
Retrieve -> Model -> Decision
  |-> FinalAnswer -> VerifyAnswer -> Complete
  `-> DurableLocalWriteAction -> Waiting
        |-> approved -> explicit resume -> VerifyWrite -> Complete/Fail
        `-> denied -> Fail with ApprovalDenied
```

The Action branch preserves the existing M5 -> M6 -> M10 -> M6.1 path. Resume
does not repeat retrieval, model invocation, decision, or the action callback.
Approval consumes no ToolCall; execution reserves the ToolCall only after valid
approval and restore validation.

## Strict Model Output

The model response must be exactly one of:

```json
{"final_answer":"...","citations":["evidence-id"]}
```

```json
{"action":{"tool":"workspace_write_file","arguments":{"relative_path":"...","content":"..."}}}
```

Mixed branches, extra root fields, duplicate keys, prose, Markdown fences,
malformed JSON, and oversized output fail closed. The final answer is limited
to 8 KiB and eight citations. Every citation must name an `EvidenceId` in the
current `EvidenceSet`; application code maps it to trusted bounded provenance.

For an Action response, M13 preserves the original `CompletedModelInvocation`
and passes it unchanged to the existing M5 decoder and validator. M13 does not
reconstruct model JSON or become a second action-validation authority.

The OpenAI-compatible adapter has opt-in trusted request defaults for bounded
JSON object output. The tested Qwen deployment additionally uses the explicit
`chat_template_kwargs.enable_thinking=false` option. These settings do not
relax the provider-neutral strict decoder.

## Knowledge Grounding

M13 reuses M8 without adding ingestion, embeddings, or storage. Trusted daemon
configuration selects OCPP, Standards, federated RequireAll, or federated
AllowPartial routing. The rendered untrusted evidence now includes its
provider-neutral `EvidenceId`, enabling strict citation binding without
trusting model-generated source metadata.

The real smoke run used the populated standards-mcp backend. The inspected OCPP
deployment had no indexed corpus at test time, so it was not used for the final
real scenarios. This is a deployment-data limitation rather than a runtime
fallback or routing change.

## Daemon Configuration and Readiness

The daemon requires trusted model configuration through `ELA_MODEL_BASE_URL`,
`ELA_MODEL_ID`, `ELA_MODEL_PROVIDER_LABEL`, and `ELA_MODEL_AUTH`. Bearer mode
also requires `ELA_MODEL_BEARER`. Explicit no-auth mode is accepted only for a
loopback endpoint. `ELA_MODEL_DISABLE_REASONING=true` applies the reviewed
Qwen/llama-server request option; invalid values fail startup.

Knowledge configuration fixes `ELA_KNOWLEDGE_ROUTE` and the corresponding OCPP
URL or standards executable/argv/environment. Startup probes model and knowledge
readiness before registering workflows.

ReadOnly registration requires persistence, required audit, model, knowledge,
and graph/configuration readiness. LocalWrite registration additionally
requires a complete trusted workspace, Bubblewrap path, static worker,
action-seal key and key ID, stable `WorkspaceBindingId`, and successful M6.1
capability probe. Missing or failed LocalWrite readiness omits only the
LocalWrite workflow; ReadOnly remains available. There is no silent downgrade.

## Results and Observability

`ApplicationResultV1` carries bounded volatile application results outside
AgentEvent, audit, and checkpoints: a final answer with trusted citations,
LocalWrite completion with `ToolCallId`, or approval denial. Application-result
loss after restart remains an explicit non-guarantee.

`ServiceEventV2` projects metadata-only workflow, graph, knowledge, model,
action, approval, tool, status, timing, and budget information. It does not
expose prompts, raw model output, evidence content, action content, ToolResult
payloads, capsule material, credentials, or audit internals.

## Real Smoke Environment

The completed smoke run used:

- local `llama-server` through `http://127.0.0.1:18080/v1` over an SSH tunnel;
- model `unsloth/Qwen3.8-27B-GGUF:UD-Q6_K_XL`;
- explicit loopback no-auth mode;
- the real standards-mcp backend and its populated ISO/standards corpus;
- Docker image `enterprise-local-agent-m6-cert:2541e5c` for write scenarios;
- Bubblewrap 0.11.2 and the static PIE LocalWrite worker;
- test-created temporary persistence and disposable write workspaces.

Secrets, prompts, evidence text, action content, and capsule plaintext were not
printed. The repository was never used as the LocalWrite target.

## Smoke Results

### ReadOnly

- Run `7b5efc5a-5d43-57fc-a1f4-4e187e6d2833` completed.
- Retrieval `5783830f-4504-4dad-9fb2-d91b89bdbf48` returned eight evidence
  items and 894 content bytes.
- Model invocation `09ee9b06-170c-4fdb-9cdb-95367e1209c4` returned strict JSON.
- The 214-byte answer bound two citations to current Standards evidence IDs.
- Budget usage was one model call and four of eight graph steps; wall time was
  approximately 32.4 seconds.

### Denied LocalWrite

- Run `e58d090e-464b-5103-9d74-2533edaf6625` reached durable Waiting.
- The exact action passed M5 proposal, validation, binding, and M10 sealing.
- A bounded trusted preview was regenerated after service reconstruction.
- Denial resumed the same graph node attempt, dispatched zero tools, and left
  the target absent.
- Usage was one retrieval, one model call, one approval request, zero ToolCalls,
  and five of eight graph steps; test wall time was approximately 27.1 seconds.

### Approved LocalWrite

- Run `1dda2913-ff5c-5fe7-8848-49522d9df5de` survived service reconstruction.
- Wait `022db7ca-c71f-4cca-80dc-9977b4a299ae`, proposal
  `bea0de20-391f-453c-a6fb-fb4064320a77`, and ToolCall
  `e3435326-4ffb-40aa-a693-407098e7da31` remained correlated.
- Retrieval, model, decision, and action callback were not repeated.
- Capsule and authority revalidation succeeded before M6 required audit and
  M6.1 dispatch.
- One contained dispatch wrote the expected file only in the disposable
  workspace, then VerifyWrite reached terminal success.
- Usage was one model call, one approval request, one ToolCall, and six of eight
  graph steps; test wall time was approximately 12.6 seconds.

This is evidence of one observed dispatch in the tested run, not an exactly-once
external-effect guarantee.

## Verification

The completed worktree passed:

- `cargo fmt --all -- --check`;
- strict workspace Clippy with all targets and features;
- workspace tests with all features, including SQLite fault injection and
  architecture/dependency boundary checks;
- the three explicitly invoked real-environment smoke tests;
- `git diff --check`;
- Codebase Memory blast-radius review;
- explicit M6.1 Linux certification with Bubblewrap 0.11.2 and the static
  worker, including escape protection and process-tree reaping.

The certification reported Landlock `PartiallyEnforced`; Landlock remains
optional defense-in-depth and is not overstated as the mandatory boundary.

## Fixes Exposed by Real Smoke

The real run identified two integration defects without changing architecture:

1. Qwen initially spent the bounded request on reasoning and later emitted an
   incorrectly flattened action shape. Opt-in bounded JSON output, explicit
   reasoning disablement, and a precise nested-envelope prompt corrected model
   reliability while preserving strict rejection semantics.
2. Grounded evidence did not expose `EvidenceId`, making the required citation
   contract impossible for a real model. Evidence rendering now includes that
   bounded provider-neutral ID, with a regression test.

## Guarantees and Limits

M13 demonstrates a real governed path from service input through enterprise
knowledge and a local OpenAI-compatible model to either a bounded cited answer
or exact contained LocalWrite after durable approval and restart. Existing M5
through M12 authority, recovery, audit, budget, and containment boundaries are
unchanged.

M13 does not claim prompt-injection immunity, exactly-once side effects,
durability of final answer content, OCPP corpus readiness, generic provider
format reliability, or production identity/RBAC. Real tests are ignored during
normal CI and require explicit trusted configuration. A write completed before
a crash is never automatically repeated; ambiguous continuation retains the
existing manual-reconciliation semantics.

## Forward Constraints

- Keep both workflow graphs immutable and server-selected.
- Keep M5 as the sole Action-envelope validation authority.
- Keep evidence and model output out of events, audit, and checkpoints.
- Never register LocalWrite without complete M6.1 and action-seal readiness.
- Keep real smoke tests explicit and use only disposable write workspaces.
- Preserve the observed-output distinction from exactly-once execution.
