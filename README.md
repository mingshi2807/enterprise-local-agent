# Enterprise Local Agent

Local-first Rust agent runtime with an enterprise execution harness, fixed-loop
and graph orchestration, replaceable model/provider adapters, governed
model-proposed actions, durable recovery, enterprise knowledge integration,
and bounded human-in-the-loop approval.

## M5 action planning

The Plan phase treats model output as untrusted data. It accepts one strict
provider-neutral JSON envelope, assigns a runtime `ActionProposalId`, validates
the exact tool name and arguments against the registered Draft 2020-12 schema,
and produces a single-use `ValidatedAction`. The Act phase then binds a fresh
`ToolCallId` and delegates policy, budget, audit, and execution to
`ExecutionHarness`.

A proposal is not a `ToolCall` and does not authorize execution. Rig/OpenAI
tool execution, native provider tool calls, retries, and heuristic JSON or
markdown extraction are not enabled.

## M6 approval and containment boundary

M6 keeps capability policy, explicit approval, and technical containment as
independent authorities. ReadOnly actions remain executable without approval.
LocalWrite actions require `M6ApprovalPolicy::RequiresApproval`, a matching
approval decision for the exact validated action, required security audit
records, and a separately registered `ContainedToolPort`. ExternalWrite and
Privileged actions remain non-executable even if a custom policy is faulty.

`ContainedToolPort` is a trusted adapter contract; implementing the Rust trait
alone does not prove operating-system isolation. M6.1 adds the Linux-only
`LinuxWorkspaceWriteTool` for one operation: `workspace_write_file`. It is
usable only after its production capability probe verifies a non-setuid
bubblewrap with FD binding, the self-contained worker, namespaces, no external
network, descriptor/environment isolation, and mandatory `openat2()` flags.
Unsupported hosts fail closed before tool registration.

The worker receives only a final basename and at most 4 KiB of approved UTF-8
content. The trusted parent resolves the target parent beneath the configured
workspace FD and exposes only that directory as writable. Landlock is reported
and used as optional defense-in-depth; it is not part of the mandatory M6.1
availability decision.

The first certified target requires an immutable, statically linked Linux ELF
worker (executable with no write bits, for example mode `0555`). Run the
fail-closed host certification explicitly after installing bubblewrap 0.11.2+
and that worker artifact:

```bash
ELA_M6_1_BWRAP=/usr/bin/bwrap \
ELA_M6_1_WORKER=/absolute/path/enterprise-local-write-worker \
CARGO_HOME=/tmp/enterprise-local-agent-cargo \
cargo test -p agent-containment-linux --all-features \
  --test linux_certification -- --ignored --exact production_linux_security_certification
```

The probe tests required behavior and does not trust the version string alone.
Invoking certification on an unsupported host is a test failure, not a skip.

## M7 durable recovery

M7 journals metadata-only events and quiescent checkpoints through the
provider-neutral `RunPersistencePort`. The SQLite adapter remains outside the
harness. Recovery validates identity, versions, sequence continuity, checksums,
and the event-chain digest, then replays the same deterministic reducer used at
runtime. Replay never invokes model, approval, tool, containment, or other
external ports.

## M8 enterprise knowledge integration

M8 integrates the existing OCPP RAG/KAG and standards-mcp backends behind the
provider-neutral `KnowledgePort`. It does not add ingestion, embeddings,
indexing, or vector storage. Trusted configuration selects one backend or a
deterministic federation; model text cannot select endpoints, processes, or MCP
operations.

The OCPP adapter uses its structured read-only `GET /search` API. The inspected
OCPP MCP tool returns Markdown-only results and does not provide reliable
structured error signaling. The standards adapter uses only the fixed
`search_standards_kag` MCP operation over bounded stdio JSON-RPC. Evidence is
bounded, provenance-preserving, and explicitly supplied to models as untrusted
data. It receives no policy, approval, tool, or containment authority.

M8 recovery persists only retrieval correlation, route, query digest/size,
evidence references/counts, available snapshot metadata, truncation/degradation,
and a manifest digest. Replay is inert. An explicitly retrieval-restartable
program may reconstruct the same query and route and perform a new read after
restart; without a backend snapshot this is fresh retrieval, not historical
replay.

## M9 deterministic graph engine PoC

M9 adds `agent-graph` as a provider-neutral orchestration sibling of
`agent-loop`. It accepts only an immutable validated DAG with typed nodes and
transitions. Graph callbacks receive narrow capability-specific facades that
delegate to `ExecutionHarness`; graph code has no direct access to model,
knowledge, tool, approval, containment, policy, registry, provider, or MCP
ports.

Graph routing is deterministic for a typed callback outcome. Application
callbacks are trusted in-process code and are not assumed pure or deterministic.
Every node consumes an independent `GraphSteps` budget reservation before its
callback, with an implementation ceiling of 64 and a default budget of zero.
The PoC flow is `Retrieve -> Model -> Action -> Verify`, ending at `Complete` or
`Fail`; it reuses M8 retrieval and the existing M5/M6/M6.1 action path.

M9 introduced metadata-only graph position, attempt IDs, transitions, program
version, and graph digest in event schema 8 and checkpoint schema 3. M10
advances the current versions to event schema 9 and checkpoint schema 4. Replay
never invokes graph callbacks or external ports. Retrieval may restart only as
a new fresh read under the explicit restart contract. Model, Action, and Verify
state is never reconstructed; unresolved external effects retain M7 manual-
reconciliation semantics.

## M10 durable graph pause/resume and HITL

M10 adds intentional quiescent `RecoveryDisposition::Waiting`, distinct from
both resumable computation and an unresolved external effect. Durable approval
is an explicit trusted graph/workflow configuration for only the existing
bounded `workspace_write_file` LocalWrite. Model output cannot select this
mode, and immediate M6 `ApprovalPort` behavior is unchanged.

Before a wait becomes visible, the harness validates the exact action, reserves
one ApprovalRequests unit, records the required ApprovalRequested audit, seals
the action, and atomically persists `GraphSuspended` with a CAS-versioned
approval row. SQLite stores only ciphertext and bounded correlation metadata;
events, checkpoints, and audit records contain no action path or content.

`LocalWriteActionCapsuleV1` is encrypted by the harness-owned `ActionSealPort`.
The local adapter uses XChaCha20-Poly1305 and authenticates the run/session,
graph version and digest, node attempt and wait, approval/proposal/tool IDs,
`ActionDigest`, `WorkspaceBindingId`, derived `ToolContractDigest`, and capsule
version. Deployment supplies key material outside SQLite.

After restart, an explicit approval-view operation decrypts and validates the
capsule and regenerates a trusted preview containing only the operation,
relative target, and content byte count. Recording Approve or Deny is a
persistence-only CAS operation and never executes a tool.

Explicit approval resume constructs a fresh `RunContext`, revalidates every
binding, performs exact tool lookup and schema validation, recomputes the action
digest, and rechecks policy, required audit health, containment, and budgets.
It then dispatches the exact action through the existing M6/M6.1 path. Resume
does not invoke the model, retrieval, planning, or the graph Action callback,
and it does not consume another ApprovalRequest or GraphStep. Denial consumes
zero ToolCalls.

Replay never decrypts capsules. `ToolInvocationStarted` without a trustworthy
terminal event remains `ManualReconciliationRequired`. A completed write that
crashes before graph continuation is never repeated; if required transient
state was lost, recovery remains manual/non-resumable. No exactly-once or
audit/SQLite cross-system transaction guarantee is claimed.

## M11 governed MCP integration

M11 adds `agent-mcp-adapters` for governed executable MCP tools. MCP remains
transport and discovery only. Trusted configuration fixes the server ID,
absolute executable, argv, explicit environment, allowlisted remote tool,
trusted local name and description, capability, and expected definition
fingerprint. Model output cannot select an MCP server, endpoint, executable, or
operation.

M11 supports only bounded newline-delimited UTF-8 JSON-RPC over stdio and pins
MCP revision `2025-06-18`, which was negotiated by both configured enterprise
servers during implementation. Discovery performs bounded `tools/list`
pagination, validates the exact allowlist and existing M5 schema profile, and
compares a deterministic SHA-256 definition fingerprint. Every invocation uses
a fresh process and repeats the protocol, definition, and fingerprint checks
before `tools/call`; definition drift dispatches no remote tool call.

Executable MCP operations still follow `ToolDefinition -> ActionProposal ->
ValidatedAction -> ExecutionHarness -> policy/budget/audit -> ManagedToolPort`.
Only trusted-configured ReadOnly tools can dispatch. MCP LocalWrite cannot call
the server and remains containment-unavailable; ExternalWrite and Privileged
remain denied. Existing M8 knowledge retrieval and M6/M6.1 LocalWrite
governance are unchanged.

`ManagedToolInvocation` gives the harness explicit process lifecycle ownership.
The harness durably records `ToolInvocationStarted` before spawning the managed
invocation, and cancellation or deadline terminates and reaps its process group.
An unresolved call recovers as `ManualReconciliationRequired`; replay performs
no MCP spawn, discovery, or invocation.

M11 accepts bounded text and structured JSON results. MCP `isError` maps to a
domain failure, while protocol, process, framing, correlation, timeout, and
drift failures remain sanitized infrastructure failures. Rich content and tools
advertising `outputSchema` are rejected in this milestone. No result content is
stored in AgentEvent or audit records.

A configured MCP executable is trusted infrastructure. ReadOnly mapping limits
what the runtime requests but does not sandbox the executable or prove it
cannot misbehave. M11 provides no HTTP MCP transport, retry/reconnect loop,
dynamic installation, output-schema support, or control over independently
daemonized descendants.

## M12 agent service API

M12 adds a provider-neutral application boundary for local clients:

```text
client
  -> agent-service-http
  -> agent-service
  -> trusted graph or loop workflow
  -> ExecutionHarness
```

`agent-service` provides session creation, idempotent run start, status and
event reads, active-run cancellation, durable Waiting discovery and preview,
Approve/Deny submission, explicit resume, and durable Waiting abort. A trusted
server-side `WorkflowId` selects a fixed composition. Clients cannot choose a
model, tool, MCP server, endpoint, policy, approval implementation, or
containment adapter.

The passive `RunReadPort` exposes bounded run summaries, verified event pages,
and Waiting records without exposing SQLite or granting mutation authority.
`ServiceEventV1` is a stable metadata-only projection rather than a serialized
`AgentEvent`; it contains no prompts, arguments, evidence, tool results,
approval previews, capsules, ciphertext, credentials, or audit internals.

M12 transport is HTTP/JSON plus SSE. The daemon listens on a mode-0600 Unix
socket by default. Optional TCP is restricted to loopback and requires bearer
authentication plus exact Host and Origin allowlists; wildcard CORS and
non-loopback listeners are rejected. Request bodies, run inputs, active runs,
concurrent commands, event pages, and SSE connections are bounded. SSE uses
`EventSequence` as its cursor, reads durable catch-up pages before polling for
new events, and disconnects slow clients instead of blocking runtime execution.
A client disconnect never cancels a run.

`RunSupervisor` permits one active run per session, reserves capacity before
spawn, and uses deterministic run IDs derived from a client start-request ID so
transport retries cannot create a second run. Startup acquires exclusive data-
directory ownership, discovers durable runs without invoking external ports or
decrypting capsules, and exposes their exact M7/M10 recovery dispositions.

Approval clients submit only the wait ID, expected row version, and Approve or
Deny. The harness loads and verifies all M10 security bindings internally.
Aborting a Waiting run atomically terminalizes it as Cancelled, consumes the
wait, executes zero tools, and causes later stale decisions to fail closed.
Persistence remains separate from required M6 audit.

The initial daemon registers only the fixed `service-health` workflow; reviewed
production graph/loop compositions remain deployment work. The service and
lower M10 layers are tested independently, but the complete HTTP Waiting ->
Approve -> resume -> real contained LocalWrite path is not yet transport-level
certified. Future ACP and Tauri/IDE adapters must reuse `agent-service` and may
not acquire runtime authority.

## Deterministic demonstration

The default CLI path is network-free. It uses `RigModelAdapter<FakeRigModel>`
to return a strict action envelope, then validates and executes the
deterministic ReadOnly fake tool through the harness:

```bash
cargo run -p agent-cli
```

It prints only run identifiers, provider label, loop progression, final status,
budget usage, and audit-degraded status.

An explicit governance-only demonstration is also available:

```bash
cargo run -p agent-cli -- --demo-local-write-fake-containment
```

That mode uses scripted fake approval and a test fake contained executor. It
does not write files, launch processes, use a network, or provide OS isolation;
the CLI labels both fakes prominently.

## OpenAI-compatible live mode

Live provider access is explicit and is never selected by default:

```bash
ELA_OPENAI_COMPAT_BASE_URL=http://127.0.0.1:8000/v1 \
ELA_OPENAI_COMPAT_MODEL=your-deployed-model-identifier \
ELA_OPENAI_COMPAT_API_KEY=your-bearer-credential \
ELA_OPENAI_COMPAT_LABEL=local-qwen \
cargo run -p agent-cli -- --live-openai-compatible
```

The model identifier is opaque configuration. It may be a concrete deployment,
an enterprise-gateway alias, or a virtual-model identifier.

HTTP is accepted only for `localhost` or an IPv4/IPv6 loopback address. All
other endpoints require HTTPS. The base URL must be an API root such as `/v1`,
not `/v1/chat/completions`. Bearer authentication is mandatory in M4.

The CLI does not print prompts, model responses, credentials, full endpoint
URLs, or model identifiers. Production compositions must not enable TRACE-level
Rig/provider logging without first reviewing the exact pinned Rig version for
content leakage.

The live mode uses the same M5 `ActionProgram`; only model composition changes.
If a live model returns prose, code fences, malformed JSON, or an invalid
action, preparation fails closed without extraction heuristics.

The M4-M6.1 layers themselves intentionally do not provide readiness calls,
retries, proxy settings, custom certificate authorities, mTLS, streaming,
structured output, model fallback, native provider tool calls, provider-driven
tool execution, enterprise identity, or durable approval. M10 adds durable
approval only for its explicitly configured `workspace_write_file` graph node;
it is not a generic approval queue or workflow engine. M6.1 containment remains
Linux-only and is not an arbitrary tool sandbox. M11 enables only configured
ReadOnly MCP tool requests and does not make MCP a policy, approval,
containment, or execution authority.
