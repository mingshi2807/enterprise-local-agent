09.08.2026

# M0

## 1. Final directory tree

  .
  ├── Cargo.toml
  ├── Cargo.lock
  ├── rust-toolchain.toml
  ├── apps/
  │   └── agent-cli/
  │       ├── Cargo.toml
  │       └── src/main.rs
  └── crates/
      ├── agent-core/
      │   ├── Cargo.toml
      │   └── src/
      │       ├── lib.rs
      │       ├── budget.rs
      │       ├── capability.rs
      │       ├── event.rs
      │       ├── ids.rs
      │       ├── model.rs
      │       ├── run.rs
      │       └── tool.rs
      └── agent-harness/
          ├── Cargo.toml
          └── src/
              ├── lib.rs
              ├── context.rs
              ├── policy.rs
              ├── ports.rs
              └── registry.rs

  The workspace uses Edition 2024, resolver 3, centralized dependencies, and stable Rust with rustfmt and Clippy components in rust-toolchain.toml.

## 2. Public types

  agent-core exports:

- IDs: RunId, SessionId, ToolCallId
- Budgets: RunBudget, BudgetUsage, BudgetDimension
- Run state: RunStatus, RunOutcome, RunFailureKind
- Capabilities: CapabilityKind
- Model contracts: ModelRequest, ModelMessage, ModelRole, ModelResponse, ModelOutputPart, TokenUsage
- Tool contracts: ToolName, ToolDefinition, ToolSchema, ToolCall, ToolInput, ToolOutput, ToolResult, ToolDomainFailure
- Events: AgentEvent, AgentEventKind, EventSchemaVersion, EventSequence

  The complete surface is re-exported from crates/agent-core/src/lib.rs:11.

  agent-harness exports RunContext, policy/authorization types, ToolRegistry, budget/context errors, and port errors from crates/agent-harness/src/lib.rs:8.

## 3. Public port traits

  Implemented in crates/agent-harness/src/ports.rs:6:

- ModelPort
- ToolPort
- AuditSink
- PortFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>

  They are object-safe, receive no RunContext, and use no Tokio or async-trait.

## 4. Budget semantics

  crates/agent-harness/src/context.rs:74 implements exact pre-operation reservation:

- If usage >= limit, deny without incrementing.
- Otherwise increment first, then return success.
- Zero model/tool limits deny immediately.
- Elapsed checking denies when supplied elapsed time is >= max_elapsed.
- No clock abstraction or step/iteration budget exists.

## 5. Capability-policy semantics

  crates/agent-harness/src/registry.rs handles only validated metadata, uniqueness, registration, and lookup.

  crates/agent-harness/src/policy.rs:9:

- Allows ReadOnly.
- Denies LocalWrite, ExternalWrite, and Privileged.
- Operates independently of the registry.
- Produces a typed PolicyDenial before any future tool invocation.

## 6. Event/audit design

  crates/agent-core/src/event.rs:47 contains only:

- Schema version
- RunId
- Deterministic EventSequence
- Typed AgentEventKind

  There are no timestamps, raw prompts, model responses, tool payloads, credentials, or provider error strings. Payload-bearing model types omit Debug; tool payload debug output is explicitly redacted.

  AuditSink is only a port contract in M0 and is not invoked.

## 7. Dependency graph

  agent-cli ──► agent-harness ──► agent-core
       └────────────────────────► agent-core

  cargo tree --workspace confirmed no reverse dependency. tracing and tracing-subscriber are CLI-only; agent-harness has no tracing dependency.

## 8. Test results

  cargo test --workspace --all-features passed:

- agent-core: 13 tests
- agent-harness: 9 tests
- Total: 22 passed, 0 failed
- Doc tests: passed

  The CLI smoke run also completed successfully without invoking models or tools.

## 9. Clippy and formatting

- cargo fmt --all -- --check: passed
- cargo clippy --workspace --all-targets --all-features -- -D warnings: passed
- cargo tree --workspace: passed
- Independent read-only code review: 0 findings

## 10. Deliberately deferred

  No Rig, Tokio, graph engine, graph-flow, petgraph, MCP, RAG, vector database, network stack, HTTP server, persistence, Qwen/OpenAI provider, identity/UserId, clock, actual model invocation, actual tool
  invocation, approvals, or deterministic loop was added.

## 11. Deviations

  No architectural deviations. For compactness, the three port traits share one ports.rs module rather than separate files; their approved ownership and contracts are unchanged.

  No commit was created.

# M1

• Implemented M1 without committing.

  1. Final changed tree

  Cargo.toml
  Cargo.lock
  apps/agent-cli/
    Cargo.toml
    src/main.rs
  crates/agent-core/src/
    event.rs
    ids.rs
    lib.rs
    run.rs
  crates/agent-harness/
    Cargo.toml
    src/
      audit.rs
      context.rs
      error.rs
      execution.rs
      execution_tests.rs
      lib.rs
      ports.rs
      registry.rs
      testing.rs

  The pre-existing modification to docs/proposal/proposal.md was left untouched.

  1. ExecutionHarness API

  crates/agent-harness/src/execution.rs:28 exposes:

  ExecutionHarness::new(...)
  start_run(&mut RunContext) -> Result<RunCancellationHandle, HarnessError>
  invoke_model(&mut RunContext, ModelRequest) -> Result<ModelResponse, HarnessError>
  invoke_tool(&mut RunContext, ToolCall) -> Result<ToolResult, HarnessError>
  complete_run(&mut RunContext)
  fail_run(&mut RunContext, RunFailureKind)
  cancel_run(&mut RunContext)

  All guarded operations retain single-owner &mut RunContext semantics.

  1. RunCancellationHandle

  crates/agent-harness/src/context.rs:156 is opaque, cloneable, and exposes only:

  request_cancel()
  is_cancelled() -> bool

  CancellationToken never appears in the public API.

  1. Lifecycle semantics

- start_run commits Running before auditing.
- Fail-closed start audit failure terminalizes as Failed { AuditUnavailable } and returns no handle.
- Completion/failure commits terminal state before terminal audit.
- Terminal audit failure never rolls back state.
- Operations before start or after terminalization return typed lifecycle errors.

  1. Model execution sequence

  Lifecycle validation → cancellation → deadline → budget reservation → allocate ModelCallId → start audit → invoke port → completion/failure audit → sanitized result.

  Reservations are execution slots, not billing counters, and are not refunded after pre-audit failure.

  1. Tool execution sequence

  Preflight → registry lookup → capability policy → denial audit or budget reservation → invocation audit → port invocation → completion/domain-failure/adapter-failure audit.

  Denied tools are never invoked and consume no tool budget.

  1. Cancellation/deadline terminalization

  Cancellation becomes Finished(Cancelled). Deadline exhaustion becomes Finished(BudgetExceeded { Elapsed }).

  Biased tokio::select! polls cancellation before deadline. Dropping an adapter future provides local cooperative cancellation only; it does not prove remote work stopped.

  1. Audit failure semantics

  crates/agent-harness/src/audit.rs:5 implements FailClosed and FailOpen.

- Pre-invocation fail-closed: NotInvoked.
- Post-invocation fail-closed: InvocationStarted.
- Committed lifecycle failure: StateCommitted.
- Fail-open marks RunContext::audit_degraded() and preserves the primary result.
- Safety terminalization always wins over terminal audit failure.
- Audit writes have bounded timeouts.

  1. Tool registry

  crates/agent-harness/src/registry.rs:8 binds:

  ToolName → ToolBinding(ToolDefinition, Arc<dyn ToolPort>)

  It handles registration, uniqueness, validated metadata, and lookup only. Authorization remains in CapabilityPolicy.

  1. Event correlation

  crates/agent-core/src/event.rs:92 advances the event schema to version 2.

- Model events correlate through the new ModelCallId.
- Tool events correlate through ToolCallId.
- Sequence numbers are allocated before audit attempts; failed writes therefore produce deterministic gaps.
- Events contain metadata only—no payloads, provider strings, timestamps, or credentials.

  1. Dependencies/features

  Added:

- Tokio: rt, macros, time.
- Tokio test-only: test-util.
- tokio-util: rt for internal CancellationToken.
- CLI enables harness test-support for the fake demonstration.

  No Tokio dependency exists in agent-core.

  1. Tests and quality gates

- cargo fmt --all -- --check: passed.
- Clippy with -D warnings: passed.
- Workspace tests: passed, 38 total.
- cargo tree --workspace: passed.
- cargo tree -p agent-core: passed.
- git diff --check: passed.
- Fake CLI demonstration: passed with final Finished(Completed) and usage model_calls=1, tool_calls=1.

  1. Dependency direction

  Verified:

  agent-cli ──► agent-harness ──► agent-core
       └────────────────────────► agent-core

  No reverse dependency. agent-core contains no Tokio, provider, Rig, graph, network, or I/O framework dependency.

  1. Deliberately deferred

  No deterministic agent loop, Rig, providers, graph engine, MCP, RAG, database, network server, Tauri, approvals, sandbox, enterprise identity, persistence snapshot, graph fan-out, or remote idempotency.

  1. Deviations

  No material deviations from the approved architecture. Audit failures are represented directly as a typed HarnessError::Audit variant carrying phase, operation, effect, and sanitized kind rather than an
  additional wrapper type.

  No commit was created.

# M1 Patchs

• A patch was required. Model/tool reservation failures previously returned BudgetExceeded without terminalizing the run.

  The narrow fix changed three files:

- crates/agent-harness/src/execution.rs:81: model/tool exhaustion now commits Finished(BudgetExceeded { dimension }), performs bounded best-effort terminal audit, then returns the original budget error.
- crates/agent-harness/src/execution_tests.rs:130: added exact regression coverage.
- crates/agent-harness/src/registry.rs:35: registration now reads and snapshots ToolPort::definition() once; the binding retains that definition and the same Arc<dyn ToolPort>.

  Verified semantics:

- Zero/exhausted model budget terminalizes with ModelCalls; model port is never invoked after exhaustion.
- Exhausted authorized-tool budget terminalizes with ToolCalls; tool port is not invoked.
- Later guarded operations fail with InvalidLifecycle.
- Policy denial leaves the run Running and consumes no tool budget.
- Fail-closed pre-audit failure leaves the reserved slot consumed but the run Running; the next failed reservation terminalizes it.
- Terminal audit failures preserve both model/tool budget-exhaustion states and mark audit degraded.
- Registry tests verify the definition is port-derived and the binding retains the same port instance.

  Quality gates:

- cargo fmt --all -- --check: passed.
- cargo clippy --workspace --all-targets --all-features -- -D warnings: passed.
- cargo test --workspace --all-features: passed, 41 tests.
- git diff --check: passed.

  No commit was created. Existing unrelated modifications under docs/ were untouched.

# M2 report

M2 is implemented and fully verified. No files were committed.

  1. Final changed tree
      - Workspace: Cargo.toml, Cargo.lock
      - CLI: apps/agent-cli/Cargo.toml, apps/agent-cli/src/main.rs
      - Core: crates/agent-core/src/budget.rs, crates/agent-core/src/event.rs, crates/agent-core/src/loop_control.rs, crates/agent-core/src/run.rs, crates/agent-core/src/lib.rs
      - Harness: crates/agent-harness/src/context.rs, crates/agent-harness/src/error.rs, crates/agent-harness/src/execution.rs, crates/agent-harness/src/execution_tests.rs
      - New agent-loop: crates/agent-loop/Cargo.toml, crates/agent-loop/src/lib.rs, crates/agent-loop/src/state.rs, crates/agent-loop/src/program.rs, crates/agent-loop/src/engine.rs, crates/agent-loop/src/
        error.rs, crates/agent-loop/src/engine_tests.rs

  2. agent-loop public API

     Exposes LoopEngine, LoopRunSummary, TerminalLoopDecision, LoopProgram, LoopEffects, LoopFuture, VerificationResult, ReflectDecision, LoopState, typed positions/terminals, and sanitized loop errors.

  3. LoopState transition model

     Pure synchronous state enforces:

     Ready → Observe → Retrieve → Plan → Act → Verify → Reflect

     Only Reflect accepts Continue, Complete, or Fail. Illegal transitions return LoopTransitionError; the engine terminalizes active runs with InvariantViolation.

  4. LoopProgram and LoopEffects

     One LoopProgram trait contains six typed phase methods and an opaque associated WorkingState. LoopEffects exposes only invoke_model and invoke_tool, both forwarding through ExecutionHarness.

     Trusted in-process programs are documented accurately: this is an architectural contract, not an I/O sandbox.

  5. Iteration budget

     RunBudget, BudgetUsage, and BudgetDimension now include iterations. ExecutionHarness::begin_iteration preflights, reserves permanently, emits IterationStarted, returns a one-based number, and terminalizes
     exhausted budgets as BudgetExceeded { Iterations }.

  6. Lifecycle ownership

     The caller starts the run and retains RunCancellationHandle. LoopEngine requires Running and owns normal complete/fail finalization.

  7. Checkpoint behavior

     ExecutionHarness::checkpoint checks lifecycle, cancellation, and elapsed deadline without consuming budgets or invoking ports. The engine calls it at deterministic phase boundaries.

  8. Harness-error handling

     After every phase future, RunContext.status() is checked before completion metadata or transitions. Existing terminal outcomes always win. Non-terminal harness errors are mapped to stable run failure
     categories and committed through fail_run; no provider retry was added.

  9. Observability and ordering

     Schema version is now 3. Metadata events cover iteration start, phase enter/complete, Reflect decision, iteration completion, and loop completion/failure. EventSequence remains authoritative; model/tool
     correlation IDs remain unchanged.

  10. Terminal decisions
  Reflect decisions remain intent until accepted by LoopState, audited, and committed by the harness. Normal Complete/Fail ordering precedes authoritative RunFinished. Committed terminal status is never reopened or replaced.

  11. WorkingState

     State remains program-owned and opaque. The engine never inspects payloads. The deterministic implementation stores provider-neutral ModelResponse and ToolResult without exposing or debugging their
     contents.

  12. Dependencies

     Direct production dependencies of agent-loop are exactly:
      - agent-core
      - agent-harness
      - thiserror

     Tokio and serde_json are dev-only. No direct serde production dependency exists.

  13. Tests and quality gates

     Passed:
      - cargo fmt --all -- --check
      - strict workspace Clippy with -D warnings
      - workspace tests: 76 unit tests passed
      - LoopState non-serialization compile-fail doctest passed
      - all four requested cargo tree commands
      - git diff --check
      - deterministic CLI smoke test: Finished(Completed), model/tool/iteration usage 1/1/1, audit not degraded
      - independent read-only review: no blocking, high, medium, or low findings

  14. Deliberate deferrals

     No Rig/providers, network, RAG/retrieval port, MCP, graph framework, persistence/checkpoints, approvals, sandboxing, identity, Tauri, retries, parallel phases, fan-out, or multi-agent behavior was
     introduced.

  15. Deviations

     No architectural deviations. LoopFailed metadata uses shared RunFailureKind, allowing stable model/tool/audit/internal and loop-specific categories without moving ReflectDecision or VerificationResult into
     core.

15.08.2026

# M3 implementation report

• M3 is implemented, fully verified, and remains uncommitted. The independent final review found zero issues.

  1. Final changed tree

- Modified Cargo.toml:1
- Updated Cargo.lock
- Added crates/agent-provider-rig/Cargo.toml:1
- Added crates/agent-provider-rig/src/lib.rs:1
- Updated apps/agent-cli/Cargo.toml:1
- Updated apps/agent-cli/src/main.rs:124

  The existing modification to docs/proposal/proposal.md was preserved and was not part of M3.

  1. Exact Rig dependency

  rig-core = { version = "=0.41.0", default-features = false }

  No rig facade, rig-agent, Agent, AgentRun, AgentRunner, or Rig orchestration features are present.

  1. RigModelAdapter shape

  RigModelAdapter<M> remains generic over rig_core::CompletionModel + Send + Sync and implements the existing ModelPort.

  Type erasure occurs only at Arc<dyn ModelPort>. No Rig types escaped into core, harness, or loop APIs.

  1. Request conversion

  The adapter preserves message order and maps System/User/Assistant explicitly. It rejects empty requests and directly constructs every Rig 0.41.0 CompletionRequest field.

  preamble, tools, tool choice, model overrides, schemas, and additional parameters remain unset. Direct construction serves as a Rig upgrade compilation tripwire.

  1. Response conversion

  Rig assistant content is processed in source order. Only textual content becomes ModelOutputPart::Text.

  Raw response data and provider message identifiers are discarded after conversion and cannot be accessed through the adapter.

  1. Usage conversion

  Only input_tokens and output_tokens are mapped:

- Nonzero → Some(value)
- Zero → None
- Both zero → no TokenUsage

  No totals, cache, reasoning, or tool-use token concepts were added to core.

  1. Unsupported content

  Tool calls, reasoning, images, and mixed supported/unsupported responses fail atomically with sanitized ModelPortError::Failed. Partial text is never returned.

  1. Error mapping

  Implemented the approved HTTP mapping:

- Other HTTP 4xx → Rejected
- 408, 429, and 5xx → Unavailable
- Statusless HTTP errors → Unavailable
- URL, request, JSON, response, provider, provider-response without status, and unknown errors → Failed

  Rig error Display, Debug, bodies, URLs, and provider strings never cross the boundary.

  1. Security and telemetry

  record_telemetry_content is always false and covered by tests. Adapter and fake request-bearing types do not derive payload-exposing Debug.

  Audit and CLI output remain metadata-only.

  1. Cancellation and retries

  The adapter performs exactly one:

  model.completion(request).await

  It adds no timeout, cancellation token, tokio::select!, retry, or hook logic. ExecutionHarness remains authoritative.

  Dropping the future only stops local awaiting; it cannot prove remote provider cancellation.

  1. Fake Rig model

  A gated FakeRigModel implements the actual Rig 0.41.0 CompletionModel and supports:

- Scripted text and usage
- Scripted completion errors
- Request capture
- Invocation counting
- Pending completions
- Deterministic sanitized streaming rejection

  It contains no network client and no hidden retries.

  1. ExecutionHarness integration

  Tests prove model budget reservation occurs exactly once through the harness. Zero or exhausted model budget prevents adapter invocation. Pending calls remain governed by harness cancellation and deadline
  behavior.

  1. LoopEngine integration

  The CLI and integration test exercise:

  LoopEngine
  → LoopEffects
  → ExecutionHarness
  → dyn ModelPort
  → RigModelAdapter<FakeRigModel>
  → rig_core::CompletionModel

  The existing ReadOnly fake tool continues through ToolPort. One iteration finishes as Finished(Completed) with model/tool/iteration usage all equal to one.

  1. Dependency trees

  Reverse Rig tree:

  rig-core v0.41.0
  └── agent-provider-rig
      └── agent-cli

  agent-core, agent-harness, and agent-loop trees contain no Rig crates.

  rig-core still brings transitive Tokio, reqwest, and HTTP-related dependencies despite disabled default features; this surface remains isolated behind the adapter crate.

  1. Architecture boundary scan

  Production sources under agent-core, agent-harness, and agent-loop produced no matches for:

  rig_core
  rig::
  rig_agent

  Their manifests also contain no Rig dependencies. No production changes were required in these crates.

  1. Tests and quality gates

  All passed:

- cargo fmt --all -- --check
- Workspace Clippy with -D warnings
- Workspace tests: 93 unit tests plus 1 doctest
- Adapter tests: 17 passed
- All requested Cargo tree commands
- Architecture source and manifest scans
- Deterministic CLI run
- git diff --check
- Independent final review: zero findings

  1. Deliberate deferrals

  Deferred as required: real providers, network configuration, tool-call correlation, streaming APIs, structured output, Rig agents, retries/idempotency, RAG, MCP, persistence, graph execution, and remote
  cancellation guarantees.

  1. Deviations

  No architectural or provider-neutral contract deviations were necessary. agent-core, agent-harness, and agent-loop were unchanged. No commit was created.

  # M4 implementation report

• M4 is implemented and fully verified. The default CLI remains network-free, the live external mode was not executed, and no commit was created.

  1. Changed tree

- Modified Cargo.toml:15, Cargo.lock
- Modified crates/agent-provider-rig/Cargo.toml:9
- Added crates/agent-provider-rig/src/openai_compatible.rs:1
- Re-exported the new API from crates/agent-provider-rig/src/lib.rs:18; RigModelAdapter itself was unchanged
- Added bounded HTTP integration infrastructure:
  - crates/agent-provider-rig/tests/support/openai_server.rs:1
  - crates/agent-provider-rig/tests/openai_compatible_http.rs:1

- Updated apps/agent-cli/src/main.rs:23
- Added usage/security documentation in README.md:6

  docs/proposal/proposal.md was already modified before M4 and was not touched.

  1. Rig dependency

  Exactly:

  rig-core = {
      version = "=0.41.0",
      default-features = false,
      features = ["rustls"]
  }

  No rig-agent, facade, native TLS, direct reqwest, or direct http dependency was added.

  1. OpenAiCompatibleConfig

  The provider-owned API validates and retains private:

- Url API root
- opaque model identifier
- mandatory BearerCredential
- optional ProviderLabel

  The factory returns Arc<dyn ModelPort> without exposing Rig types.

  1. BearerCredential

  crates/agent-provider-rig/src/openai_compatible.rs:23:

- Rejects empty or whitespace-only values
- Has redacted Debug
- Has no Clone, Display, serialization traits, or public secret accessor
- Has a compile-fail test proving it is not serializable

  1. Endpoint policy

  crates/agent-provider-rig/src/openai_compatible.rs:173 requires:

- HTTP or HTTPS with a host
- No URL credentials, query, or fragment
- API root rather than /chat/completions
- HTTP only for localhost, loopback IPv4, or loopback IPv6
- HTTPS for private IPs, internal DNS, and all other non-loopback hosts
- Only one optional trailing path slash is removed

  Both /v1 and /v1/ are proven to produce POST /v1/chat/completions.

  1. Provider factory

  crates/agent-provider-rig/src/openai_compatible.rs:153 performs:

  validated configuration
  → CompletionsClient builder
  → api_key
  → base_url
  → build
  → completion_model(model_identifier)
  → RigModelAdapter
  → Arc<dyn ModelPort>

  It adds no timeout, retry, fallback, readiness, or cancellation layer.

  1. Deterministic HTTP server

  The private Tokio server has:

- Maximum 8 scripted requests
- 16 KiB header and 64 KiB body limits
- Three-second accept/read/write bounds
- Content-Length only
- HTTP/1.1 only
- No chunked transfer, HTTP/2, or persistent connections
- Connection: close

  1. HTTP integration results

  Five integration tests passed, covering:

- URL/path normalization
- Bearer header and configured model
- Ordered System/User/Assistant messages
- No tools or tool choice
- Text and usage conversion
- 401/403, 408/429, and 5xx mappings
- Malformed JSON and malformed success responses
- Secret, prompt, response, and event leakage checks

  1. Full loop HTTP integration

  The full deterministic HTTP test completed:

  LoopEngine → LoopEffects → ExecutionHarness → ModelPort
  → RigModelAdapter → CompletionsClient → local HTTP server
  → ReadOnly ToolPort → Verify → Reflect(Complete)

  Final outcome was Finished(Completed) with:

- ModelCalls: 1
- ToolCalls: 1
- Iterations: 1

  1. Live CLI mode

  apps/agent-cli/src/main.rs:248 supports explicit:

  --live-openai-compatible

  Only that mode reads:

- ELA_OPENAI_COMPAT_BASE_URL
- ELA_OPENAI_COMPAT_MODEL
- ELA_OPENAI_COMPAT_API_KEY
- ELA_OPENAI_COMPAT_LABEL optionally

  Configuration is validated before start_run. The model identifier, URL, prompt, response, and credential are not printed.

  1. Security and logging

- record_telemetry_content = false remains explicitly constructed and tested.
- Provider construction errors use stable sanitized variants without raw sources.
- CLI logging is capped at INFO.
- Documentation warns that TRACE-level Rig/provider logging requires reviewing the pinned source first.
- Local future cancellation does not claim remote work cancellation.

  1. Dependency verification

  cargo tree checks passed for all requested crates.

  Inverse Rig dependency:

  rig-core v0.41.0
  └── agent-provider-rig
      └── agent-cli

  Rustls is enabled; no native-tls feature is present. Rig’s transitive Tokio/reqwest/runtime surface remains isolated behind agent-provider-rig.

  1. Boundary verification

  Source and manifest scans found no Rig/provider/network imports or dependencies in:

- agent-core
- agent-harness
- agent-loop

  None of those crates changed.

  1. Quality gates

  All passed:

- cargo fmt --all -- --check
- cargo clippy --workspace --all-targets --all-features -- -D warnings
- cargo test --workspace --all-features
  - 105 unit/integration tests
  - 2 compile-fail doctests

- All requested dependency trees
- Architecture source scans
- git diff --check
- Default deterministic CLI run

  The default CLI finished successfully with 1/1/1 model/tool/iteration usage and audit_degraded=false.

  1. Deliberate deferrals

  No readiness call, retry, proxy configuration, custom CA, mTLS, streaming, structured output, routing/fallback, no-auth mode, custom headers, model-driven tools, OVH SDK, or external live test execution was
  introduced.

  1. Deviations

  None. The approved provider boundary and existing M0–M3 contracts were preserved. No files were committed.
  
# M5 implementation reporting

M5 is fully implemented and verified. No commit was created, and the external live mode was not executed.

  1. Changed tree
      - Workspace: Cargo.toml:21, Cargo.lock, README.md:7
      - Core: new crates/agent-core/src/action.rs:7, IDs, events, tool correlation, exports
      - Harness: new crates/agent-harness/src/action.rs:14 and action_tests.rs; execution, registry, errors, test fakes, exports
      - Loop: governed action methods in crates/agent-loop/src/program.rs:96; LoopEngine itself remains unchanged
      - CLI: new apps/agent-cli/src/action_program.rs:22, updated composition/output
      - Provider: test-only fake tool fixture updated to echo the runtime ToolCallId
      - docs/proposal/proposal.md was already modified before implementation and was not altered by this work.

  2. Action domain types

     Added provider-neutral ActionProposalId, ActionProposal, and stable ActionRejectionReason. A proposal is explicitly untrusted, non-executable, and non-authorizing.

  3. Strict decoder behavior

     TextActionDecoder accepts exactly one textual output containing exactly:

     {"action":{"tool":"...","arguments":{...}}}

     It rejects prose, code fences, trailing content, missing/unknown envelope fields, multiple/mixed output parts, non-object arguments, and normalized/fuzzy tool names.

  4. Duplicate detection

     A recursive custom Serde visitor rejects duplicate keys during deserialization, including nested argument objects. ToolInput is constructed only after uniqueness and structural limits pass.

  5. Security limits

     Enforced fixed M5 limits: 16 KiB planning response, 128-byte tool name, 8 KiB arguments, depth 8, 64 object keys, 128 array elements, 256 nodes; schemas are limited to 32 KiB, depth 16, and 256 keys.

  6. JSON Schema profile and compilation

     ToolRegistry::register validates the profile and compiles once using jsonschema::draft202012::options() at crates/agent-harness/src/action.rs:277.

     Supported keywords match the approved profile, including title/description. $ref, composition, conditional, pattern, format, custom keywords, type unions, and wrong drafts are rejected. properties names and
     enum/const data are handled contextually.

  7. ValidatedAction invariants

     ValidatedAction has no public constructor, no raw-argument accessor, no Clone, and no serialization. Compile-fail doctests enforce the latter two contracts. It is consumed by value during execution.

  8. Tracked ModelCall correlation

     crates/agent-harness/src/execution.rs:86 returns opaque CompletedModelInvocation. crates/agent-harness/src/execution.rs:151 consumes it and binds the harness-generated ModelCallId to a fresh runtime
     ActionProposalId.

  9. Proposal/tool-call binding

     crates/agent-harness/src/execution.rs:206 generates a fresh ToolCallId, records ActionExecutionBound, and then delegates to existing governed tool execution.

  10. Policy and budget behavior

     Validation occurs before policy and tool budget. ReadOnly actions execute. LocalWrite, ExternalWrite, and Privileged actions validate but are denied with zero ToolCalls usage. Budget exhaustion after
     binding terminalizes as BudgetExceeded { ToolCalls }.

  11. ToolResult call-ID invariant

     Added crates/agent-core/src/tool.rs:272. Harness verifies returned IDs against requested IDs; mismatch becomes sanitized ToolPortError::AdapterFailure. Success and domain-failure results preserve exact
     correlation.

  12. Audit events

     Event schema advanced to version 4 at crates/agent-core/src/event.rs:8.

     Added metadata-only ActionProposed, ActionValidated, ActionRejected, and ActionExecutionBound. Tests cover correlation, ordering, FailClosed/FailOpen behavior, and payload sentinel absence.

  13. Deterministic ActionProgram

     Plan invokes and prepares the action, storing only ValidatedAction. Act consumes it once. Verify inspects typed ToolResult; Reflect deterministically returns Complete or VerificationFailed. Model text never
     controls loop transitions.

  14. Live-mode compatibility

     Deterministic and --live-openai-compatible modes use the same ActionProgram; only ModelPort composition changes. Invalid live prose/code fences fail safely without extraction heuristics.

  15. Dependency changes

     Added exactly:

     jsonschema = { version = "=0.49.2", default-features = false }

     It is directly used only by agent-harness; serde_json became a harness production dependency. No new crate or network/schema-resolution feature was added.

  16. Architecture boundaries

     Scans found no Rig/OpenAI/reqwest/url imports in production core, harness, or loop sources. Provider production code contains no tool execution path; its only tool-call reference rejects unsupported Rig
     response content.

     cargo tree -i rig-core remains:

     rig-core → agent-provider-rig → agent-cli

  17. Tests and quality gates

     All passed:
      - cargo fmt --all -- --check
      - strict workspace Clippy with -D warnings
      - cargo test --workspace --all-features: 130 unit/integration tests plus 4 compile-fail doctests
      - All requested dependency trees
      - Architecture scans
      - git diff --check
      - Default CLI: Finished(Completed), ModelCalls=1, ToolCalls=1, Iterations=1, audit_degraded=false

  18. Deliberate deferrals

     Native provider tool calls, Rig tools/agents, structured output, approvals, write execution, retries, recovery heuristics, MCP, RAG, graph execution, persistence, and multi-agent execution remain deferred.

  19. Deviations

     No architectural deviations. The only provider-crate modification is test-only adaptation to the new call-ID invariant. The validator also defensively reapplies structural limits so future non-text/native
     candidates cannot bypass them.

• Migration completed using the OMX Wiki workflow.

- Created the omx_wiki/enterprise-local-agent-milestone-index.md.
- Created curated dossiers for omx_wiki/m0-foundation.md, omx_wiki/m1-enterprise-harness.md, omx_wiki/m2-deterministic-loop.md, omx_wiki/m3-rig-model-adapter.md, omx_wiki/m4-openai-compatible-gateway.md, and
    omx_wiki/m5-typed-action-planning.md.

- Each dossier preserves objectives, approved decisions, implementation results, verification evidence, deferrals, Git tag/commit, and historical source ranges.
- OMX generated the searchable omx_wiki/index.md and omx_wiki/log.md.
- Wiki lint: 7 pages, zero issues, broken references, or orphan pages.
- Retrieval test successfully found the M5 action-planning decisions.
- The original proposal.md and reports_impl.md remain unchanged as historical archives.
- No commit was created; Git currently shows only the new omx_wiki/ directory.

  For M6 onward, we can maintain one compact milestone dossier containing proposal, approval refinements, implementation report, evidence, and deferrals instead of extending both large tracking logs.
