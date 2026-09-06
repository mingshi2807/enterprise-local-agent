# Enterprise Local Agent

Local-first Rust agent runtime with an enterprise execution harness, a typed
deterministic outer loop, replaceable model/provider adapters, and governed
model-proposed actions.

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

M9 persists metadata-only graph position, attempt IDs, transitions, program
version, and graph digest using event schema 8 and checkpoint schema 3. Replay
never invokes graph callbacks or external ports. Retrieval may restart only as
a new fresh read under the explicit restart contract. Model, Action, and Verify
state is never reconstructed; unresolved external effects retain M7 manual-
reconciliation semantics.

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

M4-M6.1 intentionally do not provide readiness calls, retries, proxy settings,
custom certificate authorities, mTLS, streaming, structured output, model
fallback, native provider tool calls, provider-driven tool execution,
enterprise identity, or durable approval. M6.1 containment is Linux-only and
limited to `workspace_write_file`; it is not an arbitrary tool sandbox.
