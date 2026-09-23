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

# M6 implementation report

    agent-harness/src/containment.rs:1, and the omx_wiki/m6-approval-and-containment-boundary.md:1.

    1. Policy/safety ceiling: crates/agent-harness/src/policy.rs:27 allows ReadOnly, requires approval for LocalWrite, and denies ExternalWrite/Privileged. The harness independently prevents faulty policies from
      bypassing the M6 ceiling. M0ReadOnlyPolicy remains unchanged.

    2. Approval domain and port: Added nominal ApprovalRequestId, bounded/redacted ApprovalPreview, ApprovalRequest, matching ApprovalDecision, and object-safe ApprovalPort. No identity claims, context,
      cancellation token, policy, registry, or executor leaks into the port.

    3. Approval budget: crates/agent-core/src/budget.rs:7 keeps its existing constructor. Approval defaults to zero and uses with_max_approval_requests. Exhaustion terminalizes as BudgetExceeded(ApprovalRequests).
    4. Digest/canonical encoding: Added redacted ActionDigest; the harness computes SHA-256 over the versioned domain separator, exact tool name, trusted capability, and validated arguments. Objects are key-
      sorted, arrays remain ordered, and signed, unsigned, decimal, and negative numbers have fixed vectors.

    5. Preview handling: Preview fields are bounded to 256 bytes, non-empty, control-character-free, non-serializable, and redacted. Preview failures retain stable unavailable/rejected/infrastructure categories
      and consume no approval or tool budget.

    6. Binding model: crates/agent-harness/src/registry.rs:64 creates a distinct contained binding. Arbitrary ToolPort instances cannot be promoted to containment.
    7. Required audit: LocalWrite progression uses centralized crates/agent-harness/src/execution.rs:1173, regardless of general FailOpen configuration. Degraded audit state blocks LocalWrite.
    8. LocalWrite sequence: crates/agent-harness/src/execution.rs:218 enforces binding, policy, containment, preview, approval budget, required approval events, exact decision matching, tool reservation, required
      invocation-start audit, and single contained execution.

    9. Containment contract: crates/agent-harness/src/containment.rs:11 is explicitly a trusted adapter contract—not proof of OS isolation. No production containment adapter ships in M6.
    10. Tool lifecycle: Success, domain failure, call-ID mismatch, containment failure, cancellation, and deadline all terminate through normal tool observability. The independent review’s lifecycle finding was
      fixed and re-reviewed with an approve verdict.

    11. Denial/failure semantics: Approval denial consumes one approval request, zero ToolCalls, invokes no executor, and causes the loop to fail before Verify/Reflect. Infrastructure errors remain sanitized.
    12. Cancellation/deadline: Harness-biased ordering remains cancellation → deadline → awaited operation. Pending approval and pending contained execution tests prove authoritative terminalization and terminal
      tool-event correlation.

    13. Deterministic fakes/demo: Test-support provides scripted/pending approval and fake containment without filesystem, process, or network effects. The explicit apps/agent-cli/src/main.rs:24 prints
      Containment: test fake — NO OS isolation.

    14. Dependencies: Added exactly sha2 = 0.11.0, default features disabled. Its sole direct workspace consumer is agent-harness.
    15. Architecture boundaries: Core/harness/loop production sources remain Rig/OpenAI/provider/network-free. Provider-rig gained no approval or containment authority; its tool references remain test-only
      integration coverage.

    16. Quality gates: Formatting and strict Clippy passed. Workspace tests passed: CLI 1, core 25, harness 85, loop 23, provider 24, HTTP integration 5, plus four doctests. All requested dependency trees,
      architecture scans, git diff --check, default CLI, and fake-containment CLI passed. Wiki lint reports zero issues.

    17. M6.1 deferrals: Real Linux isolation, filesystem containment, enterprise identity/RBAC, durable HITL, interactive approval, ExternalWrite/Privileged execution, rollback, native provider tools, retries,
      MCP, RAG, and graphs.

    18. Deviations: No scope deviations. Constructor compatibility was preserved with additive builders. The existing metadata-only ToolInvocationAdapterFailed event is reused for runtime interruption after a
      contained invocation starts rather than introducing another lifecycle vocabulary.

# M6.1 implementation reporting

  Implemented M6.1 without committing.

    Architecture

- Added agent-containment-linux (crates/agent-containment-linux/src/lib.rs:109), depending inward on agent-harness and agent-core.
- Only workspace_write_file(relative_path, content) exists.
- Preserved ValidatedAction → Policy → Approval → Required Audit → ContainedToolPort.
- No changes to ActionDigest, approval binding, policy ceiling, budgets, LoopEngine, or provider/Rig behavior.

    Containment

- Bubblewrap policy: user/PID/network/IPC/UTS namespaces, dropped capabilities, no_new_privs, new session, empty environment, private /tmp, private /proc, empty /dev, and no host root/home mounts.
- Only the safely resolved target parent is writable; the immutable worker FD is read-only. See crates/agent-containment-linux/src/launcher.rs:23.
- FD inheritance uses child-only close_range(CLOSE_RANGE_CLOEXEC), dup2, and fcntl. Parent CLOEXEC state is never globally changed. Unsafe code is isolated in crates/agent-containment-linux/src/fd_map.rs:24.
- Relative parents use descriptor-relative openat2() with BENEATH | NO_SYMLINKS | NO_MAGICLINKS | NO_XDEV; there is no weaker fallback. See crates/agent-containment-linux/src/path.rs:7.
- Landlock is probed and reported as fully enforced, partially enforced, or unavailable. It remains optional defense-in-depth.

    Worker And Lifecycle

- Added a bounded, versioned, one-request/one-response worker protocol. Content is limited to 4 KiB and total frames to 8 KiB.
- The worker receives only ToolCallId, final basename, and approved content. It writes a mode-0600 temporary file, fsyncs, atomically renames, and fsyncs the parent where supported. See crates/agent-
      containment-linux/src/bin/enterprise-local-write-worker.rs:221.

- Preview contains operation, relative target, and byte count. Content is absent from preview and audit.
- Added provider-neutral ContainedInvocation lifecycle ownership. Cancellation/deadline explicitly calls and awaits terminate_and_reap; it does not rely on kill_on_drop. See crates/agent-harness/src/
      containment.rs:22 and crates/agent-harness/src/execution.rs:811.

- Certification hooks verify Bubblewrap, worker, and descendant PIDs disappear. There is no rollback claim after rename.

    Verification

- cargo fmt --all -- --check: passed.
- Strict workspace Clippy with all targets/features: passed.
- Workspace tests with all features: 174 tests plus 4 doc tests passed; certification is separately invoked.
- Both existing CLI smoke paths passed.
- Cargo dependency trees and Codebase Memory architecture/coverage checks passed with no recorded indexing gaps.
- Certification includes governed M5→M6 create/replace, ToolCallId preservation, payload-free audit, traversal/absolute/symlink escape rejection, functional NO_XDEV, network/environment/home/SSH/FD probes, and
      process-tree reaping. See crates/agent-containment-linux/tests/linux_certification.rs:16.

    Host Result
    The explicit certification failed closed as required:

    mandatory M6.1 host capabilities must be available: Bubblewrap

    This host has Bubblewrap 0.6.1, owned by nobody:nogroup. Therefore LocalWrite cannot be constructed or registered. The installed Rust target also produces a dynamic development worker, which the production
    probe rejects; certification requires an immutable static ELF worker.

    Consequently, the requested production guarantee cannot yet be claimed on this host. After installing a trusted non-setuid Bubblewrap 0.11.2+ and immutable static worker, rerun the command documented in
    README.md:43. Landlock status and real process-tree certification remain unobserved here because Bubblewrap failed first.

    Systemd sandboxing is documented as deployment defense-in-depth, not a replacement for per-call FD binding and openat2(). No files were committed.

  # M6.1 Certification Linux env

    • Exact Commit Tested
    3081a0e279bcdf144b69583cd0d4f07cf71280f0
    Parent M6.1 commit: 2541e5cf5cf99297d710ae7a189b263ba368286e

    Environment / Kernel / Bwrap / Worker

  - Linux 6.8.0-138-generic, x86_64.
  - Debian 12 certification container, worker launched as UID/GID 1000.
  - Bubblewrap 0.11.2, upstream commit 1b80120ef26a28e065e67f89bfef873f13bdd317.
  - Built with support_setuid=false.
  - /opt/m6/bin/bwrap: root-owned regular file, mode 0755, SHA-256 52461e70c92497b12ebe637eceb0a651b9cc66251a39f45afb755f84dd815581.
  - Worker built with Rust 1.97.1 for x86_64-unknown-linux-musl.
  - Static PIE, no ELF interpreter, root-owned mode 0555, read-only mounted.
  - Worker SHA-256: 6c1e6a98155908d0019804422a447259110517071e5830fb8340b4cd6719fa4f.

    Capability Probe
    PASS. Functional --bind-fd/--ro-bind-fd launch succeeded. Probe reported:

  - namespaces isolated
  - network isolated
  - environment isolated
  - descriptors isolated
  - home/SSH isolated
  - capabilities dropped
  - no_new_privileges active
  - required openat2() behavior enforced
  - worker startup and bounded protocol successful

    Certification Results
    PASS: production_linux_security_certification completed with 1 passed; 0 failed; 0 ignored.

    Verified create/replace, absolute and traversal rejection, symlink and submount escape prevention, writable-boundary containment, network/environment/FD isolation, ToolCallId preservation, and the full
    M5→M6→M6.1 governed LocalWrite flow.

    Normal gates:

  - cargo fmt --all -- --check: PASS
  - strict workspace Clippy: PASS
  - workspace tests: PASS, 175 unit/integration tests and 4 doc tests
  - Codebase Memory blast-radius check found impact confined to agent-containment-linux; no M6 governance crate was modified.

    Process-Reaping Evidence
    PASS. The certification hook observed outer Bubblewrap plus at least three descendants, recorded PID/start-time identities, invoked the same terminate_and_reap implementation used by cancellation/deadline
    paths, and verified every identity disappeared before returning. No surviving worker or descendant was detected.

    Landlock Status
    Kernel Landlock ABI: 4.
    Status: partially_enforced, optional defense-in-depth only.

    Remaining Limitations

  - Native /usr/bin/bwrap remains version 0.6.1 and is not M6.1-capable.
  - Certification used an isolated privileged Docker test environment over the host kernel; bare-metal deployment must install the certified Bubblewrap/worker artifacts and rerun the fail-closed suite.
  - The certified worker includes the trusted certification-hooks feature; it is not reachable through model actions.
  - Scope remains one ≤4 KiB workspace_write_file operation with existing parents.
  - No rollback guarantee exists after atomic rename.

    Final Recommendation
    PASS for the m6.1-linux-localwrite-containment production tag pointing exactly to 3081a0e279bcdf144b69583cd0d4f07cf71280f0, scoped to environments that pass the same mandatory probe and certification suite.

  The enterprise write path is now genuinely closed:

                      UNTRUSTED
                          │
                      LLM/Qwen
                          │
                          ▼
                    ActionProposal
                          │
                      validation
                          │
                          ▼
                    ValidatedAction
                          │
                      policy ceiling
                          │
                          ▼
                      Approval
                          │
                  exact-action digest
                          │
                          ▼
                    required audit
                          │
                          ▼
                ContainedToolPort
                          │
                          ▼
                  Linux namespaces
                    + openat2()
                          │
                          ▼
                workspace mutation

  That is substantially stronger than most agent runtimes that simply expose a shell or filesystem tool to an LLM.

  Retain enterprise-local-agent-m6-cert:2541e5c as the reusable certification environment.
  It occupies about 1.35 GB and contains Bubblewrap 0.11.2 plus the musl build toolchain.
  The commit-specific worker should still be rebuilt for each certification run.

# M7 implementation reporting

  1. Changed tree: Added crates/agent-persistence-sqlite; added crates/agent-harness/src/persistence.rs:210 and crates/agent-harness/src/recovery.rs:516; extended harness, loop, event schema, CLI matching, and
      M6.1 certification tests. docs/reports/reports_impl.md and .codebase-memory/ were pre-existing and untouched. No commit created.

  2. Persistence port: ExecutionHarness optionally owns Arc<dyn RunPersistencePort>, exposing create_run, append_transition, and load_run. DTOs contain provider-neutral metadata only.
  3. SQLite schema: Append-only events, one runs record, and one replaceable checkpoints record. Event update/delete triggers enforce append-only behavior. Runs store session identity, expected sequence, and
      terminal state.

  4. Checkpoint model: Stores run/session identity, budget and usage, status, audit degradation, trusted wall-clock start, sequence, SHA-256 event-chain digest, loop position, iteration counters, continuation
      class, and unresolved-effect metadata. Runtime ports, payloads, approvals, ValidatedAction, futures, and tokens are excluded.

  5. Shared reducer: DurableRunState::apply drives both live event emission and recovery replay. Recovery verifies the entire checkpoint against deterministic prefix replay before replaying its tail.
  6. Recovery dispositions: Completed, TerminalFailure, Resumable, or ManualReconciliationRequired. Clock rollback fails closed; elapsed budget is reconstructed conservatively into a fresh process-local
      RunContext.

  7. Durable-before-effect: Model, approval, tool, and contained LocalWrite start events are persisted before required audit and dispatch. A persistence failure blocks dispatch; the dedicated test verifies zero
      model-port invocations.

  8. Unresolved effects: Any model, approval, or tool start without a trustworthy terminal event requires manual reconciliation. LocalWrite is never retried automatically.
  9. Restartable contract: RestartableLoopProgram (crates/agent-loop/src/program.rs:77) requires an explicit version and deterministic metadata-only state reconstruction. Arbitrary LoopProgram::WorkingState is
      never guessed or deserialized.

  10. Transaction guarantees: SQLite uses BEGIN IMMEDIATE, rollback journal, synchronous=FULL, sequence CAS, event append, optional checkpoint, and terminal update in one transaction. Blocking SQLite work uses
      spawn_blocking behind one per-adapter semaphore permit.

  11. Audit interaction: Persistence remains separate from the M6 audit sink. There is no cross-system atomicity claim. Crash windows between durable start, audit, and dispatch recover conservatively.
  12. Tests and crash injection: 189 workspace tests and 4 doctests passed. Coverage includes full/tail replay, identity/version/corruption failures, budgets, audit degradation, terminal restoration, every loop
      phase, unresolved effects, pure replay, restartable continuation, and transaction rollback injection. The explicit production Linux M6.1 certification also passed, including process-tree reaping and
      governed LocalWrite recovery without replay.

  13. Dependencies: Added rusqlite 0.40.2 with bundled SQLite for deterministic deployment and ABI independence. No distributed persistence framework was introduced. Dependency tree confirms agent-persistence-
      sqlite -> agent-harness -> agent-core.

  14. Guarantees/non-guarantees: Guarantees metadata-only deterministic recovery classification and no replay-driven external invocation. No exactly-once claim, generic working-state recovery, multi-process/
      distributed coordination, audit/database atomicity, or malicious-database tamper resistance is claimed. SHA-256 checks detect corruption but are not authentication.

  15. Deviations: AgentEvent schema advanced from 5 to 6 because replay requires safe RunStarted wall-clock metadata and durable AuditDegraded. M6 governance, approval binding, ActionDigest, budgets,
      containment, LoopEngine authority, and provider/Rig boundaries remain unchanged. Linux regression used Bubblewrap 0.11.2 on kernel 6.8 with a mode-0555 static PIE worker; Landlock reported
      PartiallyEnforced.

  Verification passed: formatting, strict Clippy, workspace tests, SQLite crash injection, dependency/graph checks, git diff --check, and mandatory Linux certification.

  The runtime is now:

  M0  contracts
  M1  execution harness
  M2  deterministic loop
  M3  Rig adapter
  M4  real model transport
  M5  typed actions
  M6  approval governance
  M6.1 Linux containment
  M7  durable events + recovery

# M8 implementation reporting

  1. Changed tree. Added agent-knowledge (crates/agent-knowledge/src/lib.rs) and agent-knowledge-adapters (crates/agent-knowledge-adapters/src/lib.rs). Updated core events/IDs, harness lifecycle/recovery, loop
      integration, workspace dependencies, CLI event matching, README, wiki, and the Codebase Memory ADR. No commit created; HEAD remains 4b14d15d0e8e23a651f84d1df33b9491f7680509.

  2. Exact backend contracts.
        - OCPP MCP inspection found get_ocpp_evidence_pack and search_ocpp_knowledge, but both return Markdown TextContent and encode backend failures as ordinary text. The adapter therefore uses structured GET /
          search with fixed q, top_k, max_chars, include_content=true, and include_query=false. It normalizes correlation_id plus ordered chunk IDs, document IDs, content, score, strategy, section/pages, evidence
          layer, source type, and content hash.

        - Standards uses only MCP search_standards_kag, sending query, limit, include_preview=true, provider=local, model=BAAI/bge-m3, pool_limit=20, graph_weight=0.001, and review_status=null. It consumes
          structuredContent containing retrieval mode, chunk/source provenance, section, heading, page range, chunk type, combined score, and content preview.

  3. KnowledgePort. KnowledgePort (crates/agent-knowledge/src/lib.rs:713) is provider-neutral and read-only. KnowledgeRequest contains a validated query, trusted route, deterministic byte/count limits, and
      optional snapshot requirements. Transport types do not leak through the port.

  4. Evidence model. Evidence, EvidenceSet, EvidenceSource, typed metadata, native score-system IDs, provenance references, and manifest digests are constructor-validated. Payload-bearing values use redacted
      Debug; evidence is intentionally not deserializable, preventing untrusted deserialization from bypassing bounds.

  5. Adapters. OCPP adapter (crates/agent-knowledge-adapters/src/ocpp.rs:36) uses fixed HTTP /search. Standards adapter (crates/agent-knowledge-adapters/src/standards.rs:71) implements a bounded MCP stdio subset
      with a fixed executable, no shell, env_clear, fixed operation, bounded protocol messages/output, sanitized errors, and direct child termination.

  6. Routing and merge. Trusted Single and ordered Federated routes support RequireAll and AllowPartial. Merge ordering is backend rank, configured backend priority, then stable evidence ID. Deduplication uses
      canonical reference when available, otherwise SHA-256 content digest. Native backend scores are retained but never compared across systems.

  7. Bounds and security. Enforced limits are 1 KiB query, 8 results, 256 KiB transport response, 4 KiB per evidence item, and 16 KiB total content. Retrieved content is explicitly labeled untrusted before
      grounding and receives no tool, policy, approval, or containment authority. Prompt-injection immunity is not claimed.

  8. M7 recovery. Replay never invokes KnowledgePort. Interrupted or transiently lost retrieval may be repeated only by an explicitly RestartableRetrieval program with a matching recovery version and
      deterministic reconstruction of query and route. Neither current backend exposes a trustworthy snapshot identifier, so resumed reads are explicitly fresh, not historical replay.

  9. Persistence and events. Event schema is now version 7 and checkpoint schema version 2. Metadata-only events cover retrieval started/restarted/completed/failed and model-grounding binding. Persisted data
      includes correlation, route, query digest/size, evidence references/count, snapshots when available, truncation/degradation, and manifest digest. Durable start failure results in zero backend calls.

  10. Dependencies. agent-knowledge uses futures-util, sha2, and thiserror. Adapters use pinned reqwest 0.13.4 without default features, serde, serde_json, Tokio, and url. No rmcp, database, embedding, indexing,
      vector-store, or provider dependency was added.

  11. Verification.
        - cargo fmt --all -- --check: passed.
        - Strict workspace Clippy with all targets/features: passed.
        - Workspace tests: 216 unit/integration tests and 4 doctests passed.
        - Explicit M6.1 certification: 1 passed in enterprise-local-agent-m6-cert:2541e5c; no relevant containment changes followed.
        - Cargo dependency scan confirms the approved inward boundaries.
        - Codebase Memory reports no recorded coverage gaps and a 36-symbol inbound blast radius across the expected integration modules.
        - ADR headings remain PURPOSE, STACK, ARCHITECTURE, PATTERNS, TRADEOFFS, and PHILOSOPHY.

  12. Guarantees/non-guarantees. M8 guarantees bounded fixed-operation retrieval, provenance preservation, deterministic federation, inert replay, metadata-only persistence, and unchanged M5-M7 execution
      authority. It does not guarantee backend availability, cross-backend score comparability, stable historical retrieval without snapshots, prompt-injection immunity, or cancellation of work already running
      inside a remote HTTP service.

  13. Deviations. OCPP uses its structured API instead of its weaker Markdown MCP contract. The standards adapter uses a small fixed-operation MCP client rather than a generic MCP SDK. Backend adapters are not
      wired into the default CLI because endpoints/process configuration are deployment-specific. Standards subprocess cancellation kills the directly owned process; descendant process-tree containment is not
      claimed. Pre-existing untracked .codebase-memory/ and knowledge/ directories remain excluded from the M8 deliverable.

  The architecture is now:

  M0   Contracts
  M1   Execution Harness
  M2   Deterministic Loop
  M3   Rig Adapter
  M4   Real Model Transport
  M5   Typed Actions
  M6   Approval
  M6.1 Linux Containment
  M7   Durable Recovery
  M8   Enterprise Knowledge Integration

# M9 implementation reporting

  1. Changed tree: Added agent-graph (crates/agent-graph/src/lib.rs), crates/agent-core/src/graph_control.rs, and the M9 wiki dossier (omx_wiki/m9-deterministic-graph-engine-poc.md). Extended core budgets/
      events, harness execution/recovery, workspace manifests, README, wiki indexes, CLI exhaustiveness, and one LoopEngine recovery-contract match. No commit was created.

  2. Graph domain/validation: Immutable bounded DAG supporting Retrieve, Model, Action, Verify, Decision, Complete, and Fail. Validation rejects cycles, unreachable/unknown nodes, duplicate or invalid
      transitions, terminal outgoing edges, invalid recovery modes, oversized IDs, more than 64 nodes, and more than 128 edges.

  3. Digest: SHA-256 with domain separation, explicit stable tags, length-prefixed identifiers, and sorted nodes, edges, and decision branches. Input/map ordering does not affect the digest.
  4. Effects façades: RetrieveEffects, ModelEffects, and ActionEffects delegate exclusively through ExecutionHarness. VerifyEffects and DecisionContext expose no external effects. Production source scans found
      no direct ports, registries, policy, approval, containment, provider, Rig, MCP, HTTP, SQLite, or agent-loop access.

  5. Execution algorithm: Harness preflight and durable graph-step reservation occur before each callback. Typed outcomes resolve exactly one validated transition. Completion/next-node state is persisted before
      advancing. Callback or invalid-transition failure terminalizes without retry or fallback.

  6. Graph budget: Added independent GraphSteps. RunBudget::new remains unchanged; default is zero. .with_max_graph_steps(n) explicitly enables execution and enforces the ceiling of 64.
  7. Persistence/recovery: Event schema is now 8 and checkpoint schema 3. Metadata-only graph events and checkpoint state record digest, node position, attempt IDs, recovery mode, and step usage. The existing
      harness reducer remains authoritative for both live transitions and replay.

  8. Restart semantics: Retrieve restarts only as a new fresh retrieval. Decision may resume only at a reconstructable deterministic boundary. Model, Action, and Verify are never reconstructed. Unresolved model/
      approval/tool/containment effects remain ManualReconciliationRequired. Version or digest mismatch fails closed.

  9. PoC workflow: Tested Retrieve → grounded Model → Action → Verify → Complete/Fail. A separate LocalWrite graph test proves M5 validation, M6 policy/approval/audit, containment dispatch, budgets, and
      ToolCallId correlation remain harness-owned.

  10. Dependencies: No new third-party dependency. Production agent-graph uses existing agent-core, agent-harness, agent-knowledge, sha2, and thiserror. SQLite, Tokio test runtime, and test fixtures are dev-
      only.

  11. Tests: Formatting, strict Clippy, dependency/boundary scans, git diff --check, and all 241 workspace tests plus four compile-fail doctests passed. SQLite transaction fault injection passed. M6.1
      certification passed without skips using Bubblewrap 0.11.2 and static PIE mode-0555 worker 227c6c8a...; Landlock reported PartiallyEnforced. Process-tree reaping was exercised by certification.

  12. Guarantees/non-guarantees: Routing is deterministic from typed outcomes; callbacks are trusted code and are not claimed pure or deterministic. No exactly-once effects, arbitrary state recovery, historical
      evidence reconstruction, dynamic graphs, cycles, parallel branches, or implicit retries are claimed.

  13. Deviations: The PoC application composition is represented by comprehensive integration tests rather than a new CLI mode. The existing immutable M6.1 worker artifact was reused because M9 did not modify
      worker or containment code. Codebase Memory ADR and wiki were updated and verified. The pre-existing untracked .codebase-memory/ directory remains outside the deliverable.

# M10 implementation reporting

 1. Waiting state model: Added quiescent RecoveryDisposition::Waiting, distinct from resumable and unresolved-effect reconciliation.
 2. Persistence schema/state machine: SQLite store schema v2 and checkpoint schema v4 add CAS-versioned durable approvals: Waiting → DecisionRecordedApprove/Deny → ApprovedReady → Executing → Consumed. Event
     schema is v9.

 3. Capsule/sealing architecture: Added agent-action-seal-local implementing harness-owned ActionSealPort with XChaCha20-Poly1305. LocalWriteActionCapsuleV1 supports only bounded workspace_write_file.
 4. Durable preview: Explicit view decrypts and revalidates the capsule, returning only operation, relative target, and content byte count. Content is never returned or persisted in plaintext.
 5. Restore validation: Resume verifies graph/program identity, all correlation IDs, ActionDigest, WorkspaceBindingId, derived ToolContractDigest, tool schema, capability, policy, containment binding, and audit
     health.

 6. Resume algorithm: Approval resume constructs a fresh RunContext, never reruns the Action callback, model, retrieval, or planning. Denial consumes the wait with zero tool calls.
 7. Audit/budget semantics: ApprovalRequests and GraphSteps remain charged once before suspension. ToolCalls is reserved only when execution starts. Required M6 audit remains separate from persistence.
 8. Crash handling: Missing terminal events remain ManualReconciliationRequired. A completed write without durable graph continuation is never repeated. Specialized persistence failures poison the process-local
     context and prevent dispatch.

 9. Graph integration: Added trusted DurableLocalWriteAction with typed success and denial transitions. Immediate M6 ApprovalPort behavior is unchanged.
 10. Crypto/key handling: Added chacha20poly1305 0.11.0 and zeroize 1.9.0. Keys are deployment supplied and absent from SQLite. Nonces are randomly generated and database-unique; errors and Debug output are
     sanitized/redacted.

 11. Tests: Formatting, strict workspace Clippy, git diff --check, dependency scans, SQLite transaction fault injection, concurrent decision CAS, 248 workspace tests, and four compile-fail doctests passed.
     Linux certification passed with Bubblewrap 0.11.2, Landlock PartiallyEnforced, and a root-owned mode-0555 static PIE worker (sha256:c5606ddd…). It includes the real M5→M6→M6.1→M7→M9→M10 restart/write path
     and process-tree reaping.

 12. Guarantees/non-guarantees: Exact approved LocalWrite survives restart encrypted and executes only through existing M6/M6.1 authorities. No exactly-once, audit/SQLite atomicity, complete memory erasure,
     generic durable tools, or automatic reconciliation is claimed. Native host Bubblewrap 0.6.1 remains unsupported.

 13. Deviations: None from the approved scope. The Codebase Memory ADR was replaced and verified with exactly its six established headings, including the narrow M7 encrypted-capsule exception. No commit was
     created; changes remain based on a1264ebed528c5b92861c9e96159d13ae5074a4d.

# M11 implementation reporting

1. Changed tree
     Added agent-mcp-adapters (crates/agent-mcp-adapters/src/lib.rs), its Rust fixture server and integration tests. Extended crates/agent-harness/src/ports.rs:21, crates/agent-harness/src/registry.rs:24, and
     crates/agent-harness/src/execution.rs:1527. Updated workspace manifests, lockfile, and the stored ADR. No commit created.

2. Supported MCP revision
     Pinned exclusively to 2025-06-18. Both target servers negotiated it live:
      - standards-mcp 1.23.0
      - rag-kag-ocpp 1.27.1

  2026-07-28 is unsupported by their installed SDKs, so it was not used.

  1. Transport/lifecycle
     Bounded newline-delimited UTF-8 JSON-RPC over stdio, fixed absolute executable/argv, env_clear, explicit environment, null stderr, no shell/retry/reconnect. Executables must be regular, executable, non-
     setid, and not group/world writable. Each invocation owns a process group; cancellation/deadline terminates and reaps it.

  2. Discovery/fingerprint
     Trusted McpServerId and ToolMapping define the allowlisted remote name, trusted local name/description/capability, and expected SHA-256 fingerprint. Discovery bounds pages/tools, rejects repeated cursors
     and duplicate names, canonicalizes object ordering, validates the fingerprint, then constructs ToolDefinition.

  3. Invocation-time drift protection
     Every invocation starts a fresh process, renegotiates the pinned protocol, repeats tools/list, and verifies the fingerprint before tools/call. Tested drift results in zero remote dispatches.

  4. Schema profile
     MCP input schemas pass unchanged through the existing M5 bounded schema validator. Unsupported references, combinators, conditionals, complexity, or size return UnsupportedTool. M11 rejects tools
     advertising outputSchema rather than relying on unvalidated output contracts.

  5. Capability mapping
     Only trusted-configured ReadOnly tools can dispatch. Managed LocalWrite reports containment unavailable; ExternalWrite and Privileged remain denied. MCP annotations have no authority.

  6. Result mapping
     Bounded text or structured JSON maps to successful ToolResult. MCP isError=true maps to domain failure. Protocol, process, correlation, malformed response, rich-content, timeout, and drift errors map to
     sanitized infrastructure failures. Payloads never enter events or audit.

  7. Recovery
     Existing durable ToolInvocationStarted semantics are reused unchanged. Persistence failure prevents process/tool dispatch. An unresolved MCP invocation recovers as ManualReconciliationRequired; replay
     performs no spawn, discovery, or call.

  8. Dependencies
     No MCP SDK or framework added. The new crate uses existing agent-core, agent-harness, Tokio, Serde JSON, SHA-256, thiserror, and libc. SQLite and tempfile are test-only dependencies.

  9. Tests
     All gates passed:

- formatting and strict workspace Clippy
- 265 workspace tests plus 4 doctests
- 15 adapter tests and 2 managed-harness tests
- SQLite transaction/crash tests
- Codebase Memory dependency/blast-radius scans
- M6.1 certification with Bubblewrap 0.11.2, static musl worker, process-tree reaping, and Landlock PartiallyEnforced

  1. Guarantees/non-guarantees
     The runtime guarantees governed, fingerprint-bound ReadOnly MCP requests through the harness. A configured MCP executable remains trusted infrastructure: ReadOnly classification does not sandbox the
     executable or prove it cannot misbehave. Cleanup covers the tested process group, not independently daemonized descendants. No exactly-once claim.

  2. Deviations
     2025-06-18 was selected instead of preferred 2026-07-28 based on live compatibility. HTTP, rich content, output schemas, retries, MCP LocalWrite, and production auto-registration remain unsupported.
     Deployment must supply reviewed allowlists and expected fingerprints.

# M12 implementation reporting

1. Tree
     Added agent-service (crates/agent-service/src/lib.rs:190), agent-service-http (crates/agent-service-http/src/lib.rs:126), and agent-service-daemon (apps/agent-service-daemon/src/main.rs:177). Extended
     harness recovery/read APIs and SQLite persistence.

2. Service API
     Supports session creation, idempotent run start, status, event reads, cancellation, Waiting discovery/preview/decision/resume, and durable Waiting abort.

3. Read Boundary
     Added provider-neutral RunReadPort (crates/agent-harness/src/read.rs:197). SQLite implements verified paginated run, event, and Waiting reads without exposing storage details.

4. Transport/Auth
     Axum HTTP/JSON and SSE. Unix socket is default with 0600 permissions. Optional TCP rejects non-loopback addresses and requires bearer authentication plus exact Host and Origin allowlists.

5. Run Ownership
     One active run per session, 32 active runs maximum, deterministic RunId derivation from the client start-request ID, CAS-backed resume, and exclusive data-directory locking.

6. ServiceEventV1/SSE
     ServiceEventV1 (crates/agent-service/src/lib.rs:645) is an explicit metadata-only projection. SSE uses durable sequence cursors, bounded pages/connections, polling without catch-up races, and disconnects
     slow clients without cancelling runs.

7. HITL/Abort
     Clients provide only wait ID, expected row version, and Approve/Deny. Internal bindings are loaded and verified by the harness. Abort atomically terminalizes the run as Cancelled, consumes the wait,
     executes zero tools, and rejects stale decisions.

8. Recovery/Startup
     Startup acquires exclusive ownership, loads trusted composition, discovers durable runs, and invokes recovery without external effects or capsule decryption. Waiting, Resumable, terminal, and manual-
     reconciliation states remain distinct.

9. Concurrency/Idempotency
     Duplicate start requests resolve to the same run. Concurrent start/resume conflicts fail closed. Commands have a fail-fast 32-operation bound; SSE allows 64 global and 4 per run.

10. Tests
     fmt, strict workspace Clippy, and all workspace tests pass. This includes SQLite transaction faults, M10 approve/deny/abort and no-replay tests, M11 regressions, service ownership, event projection, SSE
     reconnect/backpressure, auth, body bounds, and daemon lock/socket tests.

11. Guarantees/Non-Guarantees
     Clients cannot select execution infrastructure or bypass ExecutionHarness. No payloads, capsules, evidence, results, or audit internals enter service events. No multi-process coordination, ACP, non-loopback
     serving, RBAC, or exactly-once external-effect claim.

12. Deviations
     The daemon currently registers only a fixed service-health workflow; production graph/loop compositions must be added through reviewed server-side configuration. Mandatory M6.1 certification was run and
     failed, without skipping, because this host has Bubblewrap 0.6.1 rather than the required FD-binding-capable version. The static worker remains available, but production containment certification cannot be
     claimed on this host.

• M6.1 certification rerun: PASS.

- Environment: enterprise-local-agent-m6-cert:2541e5c
- Source: current uncommitted worktree based on 27bba306edb6bf800eb336e34906fa63e1d9c5ee
- Toolchain: Rust 1.97.1
- Bubblewrap: 0.11.2, root-owned, mode 0755
- Bubblewrap SHA-256: 52461e70c92497b12ebe637eceb0a651b9cc66251a39f45afb755f84dd815581
- Worker: static PIE musl ELF, root-owned, mode 0555
- Worker SHA-256: fb5d6066384f65b0934b46dd05fbc2f84566d0c9f580b63c312a89a35e7d2295
- Execution user: UID 1000
- Landlock: PartiallyEnforced optional defense-in-depth

  Exact certification result:

  production_linux_security_certification ... ok
  1 passed; 0 failed; 0 ignored

  The unchanged suite exercised the capability probe, containment boundaries, network/environment/FD isolation, openat2(), governed LocalWrite flow, ID preservation, and process-tree termination/reaping.

  The repository was mounted read-only inside the certification container. No files were modified and no commit was created

# M13 smoke testing report

• 1. Environment

- Commit tested: 803fe05797ea325bae6b138e875163c8c4f78822 plus uncommitted M13 fixes
- Provider: local llama-server, OpenAI-compatible
- Model: unsloth/Qwen3.8-27B-GGUF:UD-Q6_K_XL
- Endpoint: <http://127.0.0.1:18080/v1> through SSH tunnel
- Authentication: explicit loopback no-auth
- Knowledge route: StandardsMcp
- LocalWrite: disposable temporary workspace
- Containment: enterprise-local-agent-m6-cert:2541e5c, Bubblewrap 0.11.2
- Worker: 818,856-byte static PIE ELF

  1. ReadOnly Result

  PASS.

- Run: 7b5efc5a-5d43-57fc-a1f4-4e187e6d2833
- Retrieval: 5783830f-4504-4dad-9fb2-d91b89bdbf48
- Model invocation: 09ee9b06-170c-4fdb-9cdb-95367e1209c4
- Evidence: 8 items, 894 bytes
- Answer: 214 bytes, 2 valid citations
- Terminal status: Completed

  1. Denied Write

  PASS.

- Run: e58d090e-464b-5103-9d74-2533edaf6625
- Entered durable Waiting
- M5 proposal and validation events recorded
- Denial survived service reconstruction
- Tool dispatches: 0
- Target file: not created
- Terminal result: ApprovalDenied

  1. Approved Restart/Resume

  PASS.

- Run: 1dda2913-ff5c-5fe7-8848-49522d9df5de
- Wait: 022db7ca-c71f-4cca-80dc-9977b4a299ae
- Proposal: bea0de20-391f-453c-a6fb-fb4064320a77
- ToolCallId: e3435326-4ffb-40aa-a693-407098e7da31
- Same identifiers survived restart
- Model/retrieval/action callbacks were not repeated
- Observed contained dispatches: exactly 1
- File was created only inside the temporary workspace
- Terminal status: Completed

  This is observed single dispatch, not a generic exactly-once guarantee.

  1. Budgets/Timing

- ReadOnly: model 1/1, graph steps 4/8, approximately 32.4 seconds
- Denied: model 1/1, approval 1/1, tools 0/1, graph steps 5/8, test 27.1 seconds
- Approved: model 1/1, approval 1/1, tools 1/1, graph steps 6/8, test 12.6 seconds

  1. Model Compliance

  The final model responses complied with the strict JSON union:

- ReadOnly: exactly final_answer plus citations
- LocalWrite: exactly the nested M5 action envelope
- No prose, fences, <think> tags, or extra root fields

  The initial write attempt used the wrong root shape. The prompt was tightened without weakening strict parsing.

  1. Knowledge/Citations

  Standards retrieval and citation binding passed. Grounded evidence now includes the provider-neutral EvidenceId, allowing model citations to bind to trusted provenance.

  The OCPP backend was inspected but had an empty corpus, so the real tests used the populated Standards backend.

  1. Recovery/HITL

  Durable Waiting, preview, decision CAS, restart, explicit resume, capsule validation, and ID correlation passed. Replay did not call the model or knowledge backend and did not repeat the action callback.

  1. Containment

  Explicit M6.1 certification passed:

- Bubblewrap: 0.11.2
- Static worker trusted and executable
- Capability probe passed
- Create/replace and escape protections passed
- Process-reaping certification passed
- Landlock: PartiallyEnforced, optional defense-in-depth

  1. Fixes and Verification

  Code fixes covered bounded JSON output, disabling Qwen reasoning when configured, exact action prompting, evidence-ID grounding, and richer sanitized real-test assertions.

  Passed:

- cargo fmt --all -- --check
- strict workspace Clippy
- workspace tests with all features
- SQLite fault tests
- architecture/dependency tests
- git diff --check
- explicit M6.1 certification
- all three real smoke scenarios

  No commit was created. The untracked crates/agent-mvp/knowledge/ backend cache is approximately 2.2 GB and must not be committed.

  1. Recommendation

  M13 PASS.

# M14 implementation reporting

• Implemented M14 without committing.

  1. Tree

- Added agent-deployment (crates/agent-deployment/src/lib.rs:38).
- Added agent-operator (apps/agent-operator/src/main.rs:14).
- Added DeploymentConfigV1 example (docs/deployment-config-v1.example.toml).
- Extended service, HTTP transport, daemon composition, SQLite administration, and containment metadata exports.

  1. Config/Fingerprint

- Strict, bounded DeploymentConfigV1; unknown fields rejected.
- Covers listener security, storage/audit, workflows, model, knowledge, MCP, LocalWrite artifacts, seal reference, and operational limits.
- Secrets resolve from protected external files and use redacted Debug.
- SHA-256 deployment fingerprint includes security-relevant non-secret configuration only.

  1. Profiles/Readiness

- ReadOnly and LocalWrite readiness are independent.
- Model and knowledge receive bounded startup probes.
- LocalWrite registration additionally requires seal key, workspace binding, tool-contract digest, artifact hashes/trust properties, worker protocol, and M6.1 capability probe.
- LocalWrite failure leaves ReadOnly available and is never silently downgraded.
- /healthz reports lifecycle only; readiness is cached metadata.

  1. Observability

- Metadata-only fixed-bucket metrics for runs, model/retrieval/tool latency, approval decisions/wait observations, connections, and recovery classifications.
- Post-workflow status is determined by replay-only durable recovery, so Waiting is not counted as completed.
- Operational run views exclude application results and payloads.

  1. Shutdown

- Added Serving → Draining.
- New sessions, starts, resumes, and decisions are rejected while draining.
- Active cancellation handles are signalled and tracked tasks are awaited.
- Audit is flushed even when the drain deadline expires.
- Existing M7 unresolved-effect classification remains authoritative; shutdown does not manufacture Cancelled.

  1. Backup/Restore

- Added SqliteStoreAdmin using SQLite’s backup API.
- Backup requires exclusive data-directory ownership and copies audit only while quiesced.
- Manifest hashes database and audit and records build information plus compatibility-critical contracts.
- Restore requires an empty target, verifies integrity first, and cleans partial output on failure.
- Symlinked database/audit sources are rejected.

  1. Compatibility

- Fail-closed checks cover store/event/checkpoint versions, recovery contracts, graph digests, capsule version, tool contracts, key IDs, workspace bindings, MCP fingerprints, containment protocol, and artifact
    hashes.

- Different application builds are accepted when these critical contracts match.
- No automatic migration or downgrade was added.

  1. Containment

- Runtime verifies root ownership, immutable executable mode, no setuid/setgid, configured SHA-256, worker protocol, tool-contract digest, and the existing bounded M6.1 probe.
- Runtime readiness is not described as universal certification.

- config validate
- readiness
- version
- runs list/show
- reconciliation list/show
- backup create/verify
- restore verify/apply

  It has no approval, resume, policy, model, tool, or containment authority.

  1. Verification

- cargo fmt --all -- --check: passed.
- Strict workspace Clippy with all targets/features: passed.
- Workspace tests: 299 passed, plus 4 compile-fail doctests.
- SQLite transaction-fault, corruption, backup/restore, tamper, and cross-build tests passed.
- Dependency and forbidden-authority scans passed.
- Codebase Memory blast-radius and coverage checks completed with no recorded source gaps.
- Operator config-validation smoke passed.

  M6.1 certification passed without skips:

- Bubblewrap 0.11.2, SHA-256 52461e70…15581
- Fresh static PIE worker, root-owned mode 0555, SHA-256 338dc225…cddae2
- Landlock: PartiallyEnforced
- Certification: 1 passed; 0 failed; 0 ignored

  1. Guarantees/Non-Guarantees

- Deployment operations do not acquire ExecutionHarness execution authority.
- Backups are crash-consistent and integrity-checked, but hashes are not cryptographic authenticity signatures.
- Metrics contain no payloads or high-cardinality run labels.
- Approval waits spanning process restart do not have a complete duration metric because M10 intentionally persists no approval timestamp.
- No HA, multi-process coordination, online backup, model lifecycle management, or exactly-once execution claim.

  1. Deviations

- None affecting the approved authority or security architecture.
- MCP entries unused by the M13 workflows are reported as configured but degraded, rather than falsely reported ready.
- The host lacked the musl target; the certification container built the exact current worker after installing the target inside that ephemeral environment.
- No commit created.

# M15 implementation reporting

• Implemented M15 without committing.

  1. Tree
      - Added agent-identity (crates/agent-identity/src/lib.rs:1).
      - Integrated identity into service, HTTP transport, harness, SQLite persistence, deployment configuration, daemon composition, M10 capsules, and MVP tests.
      - Core contains only bounded PrincipalId (crates/agent-core/src/ids.rs:10).

  2. Principal/Authentication
      - Added VerifiedPrincipal, Human | Service | LocalProcess, trusted roles, and authentication classes.
      - Unix sockets authenticate through real SO_PEERCRED UID/GID mapping in agent-service-http (crates/agent-service-http/src/lib.rs:79).
      - Loopback bearer authentication maps to exactly one configured service principal.
      - PID, credentials, tokens, emails, and arbitrary claims are not persisted.

  3. Authorization
      - Added typed default-deny ServiceAuthorizationPolicy.
      - Every client-facing service operation receives a VerifiedPrincipal.
      - Durable ownership controls run/status/event/cancel/resume access.
      - Approval authorization is separate from ownership and supports RequesterMayApprove and RequesterMustDiffer.
      - Operator role grants inspection only, not workflow execution or approval authority.

  4. Durable Ownership
      - Session ownership is checksum-protected in SQLite through SessionOwnershipPort.
      - Run owner, requester, workflow, policy version, and policy fingerprint are atomically persisted with run creation.
      - Restarted services authorize using durable metadata rather than client-provided IDs.

  5. Legacy Strategy
      - Existing unowned M14 records are never assigned inferred owners.
      - Legacy Waiting runs are projected as ManualReconciliationRequired.
      - Normal approval/resume access fails closed; operator inspection remains available.
      - No offline ownership migration utility was added.

  6. Approval Identity
      - Durable Approve/Deny records now include actor and decision timestamp.
      - Requester identity is authenticated in LocalWriteActionCapsuleV1 AAD.
      - Exact M10 request/proposal/tool/action digest bindings remain unchanged.
      - Duplicate, stale, wrong-principal, and requester-separation rules fail closed.

  7. Audit
      - Security mutations follow:
        AuthorizationGranted → MutationRequested → durable mutation → MutationCommitted/Failed.

      - Required pre-mutation audit failure produces zero mutation.
      - Principal correlation is limited to security-relevant audit/approval metadata.
      - M6 LocalWrite audit remains separate and authoritative.

  8. Recovery
      - Replay restores durable identity metadata without authentication or authorization calls.
      - New commands authenticate and evaluate current trusted policy.
      - Replay performs no model, knowledge, tool, containment, capsule, or identity-provider invocation.

  9. Tests
      - Workspace: 310 unit/integration tests and 4 compile-fail doctests passed.
      - Covered principal validation/redaction, default deny, cross-principal isolation, delegated approval, requester separation, restart ownership, legacy Waiting, capsule requester tampering, audit ordering/
        failure, SQLite corruption, spoofed identity headers, and operator restrictions.

      - cargo fmt --all -- --check: passed.
      - Strict workspace Clippy: passed.
      - SQLite transaction-fault tests: passed.
      - Codebase Memory blast-radius and dependency scans: passed.

  10. M6.1 Certification

- enterprise-local-agent-m6-cert:2541e5c: PASS, no skip.
- Bubblewrap 0.11.2, root-owned mode 0755.
- Static PIE worker, root-owned mode 0555, SHA-256 9e6a0b61e70f36efdc93b73ea462fdb479dfe10b19c9380140a2e6b439db7dfd.
- Landlock: PartiallyEnforced.
- Full containment and process-tree reaping test: 1 passed; 0 failed.

  1. Guarantees/Deviations

- Identity authorizes service requests but cannot bypass M5 validation, M6 policy/approval/audit, M7 recovery, M10 exact-action binding, or M6.1 containment.
- No OIDC, generic IAM engine, automatic legacy migration, or distributed identity cache.
- Deployment schema advanced to version 2 for mandatory identity configuration, while the existing DeploymentConfigV1 Rust type name was retained.
- Existing untracked .codebase-memory/ and crates/agent-mvp/knowledge/ remain uncommitted.
- No commit created.

• Cleaned up the schema naming:

- Renamed DeploymentConfigV1 to DeploymentConfigV2.
- Renamed example to docs/deployment-config-v2.example.toml.
- Updated daemon, operator, tests, and README references.
- No compatibility alias retains the misleading V1 name.
- Historical M14 documentation remains unchanged.

  Verification passed: fmt, strict Clippy, full workspace tests, and git diff --check. No commit created.

# M16.0 implementation architecture

# M16.1 implementation reporting

• 1. Changed Tree

- Added apps/agent-desktop with React/Vite frontend and Tauri Rust application.
- Updated workspace Cargo.toml, Cargo.lock, and .gitignore.
- Added frontend configuration, tests, design tokens, shadcn foundation, Tauri capabilities, generated command permissions, and application icons.
- Pre-existing untracked .codebase-memory/ and crates/agent-mvp/knowledge/ were not modified intentionally.

  1. Tauri/Service Bridge

  apps/agent-desktop/src-tauri/src/service_client.rs provides:

- Unix-socket transport where supported.
- Authenticated numeric-loopback HTTP fallback.
- Named health, readiness, and version commands only.
- Five-second timeouts, 64 KiB response limits, strict response decoding, sanitized errors, and disabled redirects/proxies.
- Bearer credentials remain Rust-side, use redacted Debug, and are zeroized where practical.

  1. Capability Boundary

  apps/agent-desktop/src-tauri/capabilities/main.json grants only the three service commands.

  There are no filesystem, shell, generic HTTP, SQL, model, MCP, persistence, or containment capabilities. Global Tauri access is disabled, CSP uses connect-src 'none', and native window decorations remain
  enabled.

  1. Theme/Design Foundation

  apps/agent-desktop/src/app/App.tsx and apps/agent-desktop/src/styles/tokens.css implement:

- Semantic light, dark, and system themes.
- Neutral OKLCH surfaces, restrained accent, compact spacing, subtle borders, and visible focus states.
- Empty developer-tool shell with top bar, activity rail, navigation area, workspace, readiness panel, and status bar.
- TanStack Query providers, shadcn-compatible primitives, and Lucide icons.
- Motion is installed but intentionally unused.

  1. Service Connectivity

  Health polls every 5 seconds and readiness every 15 seconds. Version metadata is cached indefinitely. The shell distinguishes connected, degraded, unavailable, and draining states without exposing transport
  credentials or backend payloads.

  1. Tests

  Passed:

- Frontend typecheck, ESLint, 2 Vitest tests, Vite production build, and npm audit with zero vulnerabilities.
- Rust formatting and strict workspace Clippy.
- Full workspace tests with all normal tests passing.
- Four desktop bridge tests covering loopback authentication, endpoint rejection, Unix sockets, strict decoding, and response bounds.
- Tauri check and debug build with --no-bundle.
- Permission, forbidden-surface, dependency-direction, architecture, and change-impact scans.

  1. Deviations

- No shared agent-service-api crate was introduced because three narrow read-only DTOs do not yet justify changing the stable M12 HTTP boundary.
- Linux Tauri checks ran inside a disposable Debian build container because the host lacks WebKitGTK/GLib development packages.
- Distribution packaging and signing remain deferred; the verified Tauri build used --no-bundle.
- Headless Firefox screenshot capture was unavailable due the host Snap/DBus environment; DOM tests and native compilation passed.
- No commit was created.

# M16.2 implementation report

• 1. Changed UI Tree

- apps/agent-desktop/src/app/App.tsx
- apps/agent-desktop/src/app/CommandPalette.tsx
- apps/agent-desktop/src/components/ConversationWorkspace.tsx
- apps/agent-desktop/src/components/SessionSidebar.tsx
- apps/agent-desktop/src/components/ReadinessInspector.tsx
- Updated design tokens, responsive test setup, application tests, and minimum window dimensions.

  1. Layout/Components

  Implemented a conversation-first shell:

- Compact application bar.
- Collapsible conversation sidebar with mock sessions.
- Dominant central task workspace.
- Read-only composer placeholder.
- Optional readiness inspector.
- Subtle connection footer.
- Accessible command palette.

  Operational details remain confined to the inspector and status bar.

  1. Interaction/Shortcuts

- Cmd/Ctrl+K: command palette.
- Cmd/Ctrl+N: new task placeholder.
- Cmd/Ctrl+B: conversation sidebar.
- Cmd/Ctrl+Shift+I: inspector.
- Palette filtering, arrows, Enter, Escape, focus trapping, and focus restoration.
- Editing-focused inputs do not trigger pane or task shortcuts.

  1. Responsive Behavior

- Wide: full sidebar, conversation workspace, and inspector.
- Normal desktop: full three-pane layout.
- Compact, from 760 px: 48 px conversation rail, inspector closed, workspace preserved.
- Below 1041 px, an explicitly opened inspector becomes an overlay.
- Tauri minimum window changed to 760x520.

  1. Accessibility

- Semantic landmarks and labelled navigation.
- Accessible toggle states and command dialog.
- Screen-reader service-state announcement.
- Reduced-motion preference disables effective transition duration.
- Disabled composer actions accurately communicate unavailable functionality.

  1. Visual Design Decisions

  Neutral light/dark surfaces, restrained teal accent, 1 px borders, compact typography, limited radii, and almost no shadows. Motion is restricted to sidebar, inspector, and command-palette transitions. No
  gradients, decorative effects, or nested card layout were added.

  1. Tests

  Passed:

- TypeScript typecheck.
- ESLint with zero warnings.
- Eight frontend tests covering service states, draining precedence, keyboard interactions, editing safeguards, command palette, and compact layout.
- Vite production build.
- Rust formatting.
- Strict desktop Clippy.
- Four Rust service-bridge tests.
- Tauri debug build with --no-bundle.
- Capability and forbidden-surface scan; Tauri permissions remain unchanged.

  1. Screenshots

- Compact light, 800x600 (/tmp/ela-m16-2-compact-light.png)
- Normal light, 1280x800 (/tmp/ela-m16-2-normal-light.png)
- Wide dark, 1600x900 (/tmp/ela-m16-2-wide-dark.png)

  1. Deviations

- Resizable panes were not added; collapse and overlay behavior provides reliable compact handling without another interaction dependency.
- Browser screenshots show the unavailable state because a browser cannot access the Tauri command bridge. Ready, degraded, unavailable, and draining projections are covered by tests.
- Vite reports a non-failing 529 kB bundle advisory. Code splitting is deferred until a real route or conversation boundary exists.
- No Tauri capabilities or execution authority were added. No commit was created.

# M16.3 implementation report

• 1. Changed Tree

- Tauri bridge (apps/agent-desktop/src-tauri/src/service_client.rs)
- Conversation controller (apps/agent-desktop/src/queries/conversation.ts)
- Conversation workspace (apps/agent-desktop/src/components/ConversationWorkspace.tsx)
- Markdown answers (apps/agent-desktop/src/components/MarkdownAnswer.tsx)
- Updated contracts, shell, sidebar, styles, tests, dependencies, and generated Tauri ACL schemas.

  1. Service Commands

  Added five allowlisted commands:

- conversation_create_session
- conversation_start_readonly_run
- conversation_run_status
- conversation_cancel_run
- conversation_read_events

  The workflow is fixed Rust-side to enterprise-engineering-readonly-v1. Credentials and transport configuration never enter JavaScript.

  1. Conversation Flow

  The composer supports multiline editing and Cmd/Ctrl+Enter. It enforces the 8 KiB input bound, permits one active run per conversation, supports cancellation, and retries by creating a new run.

  Conversation payloads remain in the in-memory TanStack Query cache. Nothing is written to browser storage.

  1. Streaming/Events

  Bounded polling consumes strict ServiceEventV2 pages with sequence and run correlation validation. It projects only:

- Searching knowledge
- Thinking
- Verifying
- Finishing
- Reconnecting/stopping

  The terminal ApplicationResultV1::FinalAnswer remains authoritative.

  1. Citation UX

  Answers render bounded Markdown and GFM with inert model-generated links, copy-answer/code controls, and expandable citation chips using trusted service provenance.

  1. Error/Cancel Behavior

  Added sanitized states for service start failure, model/knowledge failure, malformed model output, cancellation, reconnecting, and result loss after service restart. Raw service errors and model payloads are
  never displayed.

  1. Tests

  Passed:

- Frontend typecheck and strict ESLint
- 12 frontend tests
- Production Vite build
- Strict Rust Clippy
- 6 Rust desktop tests
- Tauri no-bundle build
- cargo fmt --all -- --check
- Tauri capability scan: eight named permissions and no generic bridge

  1. Screenshots

  No screenshot was captured because no live service/model composition was started for this implementation run.

  1. Deviations

- Event subscription uses bounded cursor polling rather than a persistent WebView SSE connection.
- The service exposes no standalone session-load/list route. Existing in-memory sessions are selectable, and known runs are reloaded through status/event commands.
- Vite reports a non-failing 696 KiB JavaScript chunk warning.
- No commit was created.

# M16.4 implementation

1. Changed tree
      - Updated apps/agent-desktop/src/app/App.tsx, apps/agent-desktop/src/components/ConversationWorkspace.tsx, conversation state, contracts, styles, and tests.
      - Added apps/agent-desktop/src/components/RunInspector.tsx and apps/agent-desktop/src/features/runActivity.ts.
      - Replaced the old readiness-only inspector.

2. Activity UX
      - Live phase, elapsed time, restrained activity indicator, and Stop action.
      - Terminal runs collapse to Completed/Failed/Cancelled · duration · sources.
      - Reconnecting and stopping states remain explicit.

3. Inspector/timeline
      - Optional inspector remains closed by default.
      - Shows workflow, phase, duration, model calls/budget, graph steps, backends, citations, terminal status, and copyable IDs.
      - Lightweight Retrieve → Model → Verify → Complete timeline.

4. Event handling
      - Supports the service’s zero-based EventSequence.
      - Ignores duplicates and rejects gaps, out-of-order pages, and wrong-run events without advancing the cursor.
      - Terminal status cannot regress into transient activity.

5. Performance/bundle
      - Lazy-loaded inspector: 5.82 kB.
      - Lazy-loaded Markdown/GFM: 155.13 kB.
      - Main bundle reduced from roughly 697 kB to 543.51 kB.
      - Vite still reports the main chunk above its 500 kB advisory threshold.

6. Accessibility
      - Semantic status announcements, native controls, copy labels, reduced-motion support, and keyboard inspector toggle.
      - Compact and wide layouts remain supported.

7. Tests
      - TypeScript typecheck: PASS
      - ESLint: PASS
      - Frontend: 16 tests PASS
      - Production frontend build: PASS
      - Rust formatting: PASS
      - Containerized strict Clippy: PASS
      - Rust bridge: 6 tests PASS
      - Tauri debug build: PASS
      - Capability/command surface unchanged.

8. Screenshots
      - Not captured; no GUI-capable review session was available.

9. Deviations
      - Actual evidence count is not present in existing ServiceEventV2; the inspector explicitly displays Not exposed rather than inferring it from citation count.
      - No LocalWrite/HITL functionality added. No commit created.

# M16.5 implementation reports

1. Changed tree
      - Rust bridge: named LocalWrite/HITL commands, strict DTO validation, generated Tauri ACLs.
      - Frontend: approval contracts, controller state, inline ApprovalPanel, workflow selector, activity integration, tests.
      - Runtime governance crates remain unchanged.

2. Service commands
      - conversation_start_localwrite_run
      - approval_list_waiting
      - approval_resume_run
      - approval_abort_waiting
      - No generic HTTP, filesystem, shell, SQL, model, MCP, persistence, or containment command.

3. Approval UX
      - Compact inline LocalWrite panel.
      - Displays only operation, relative target, and byte count.
      - Supports Approve, Deny, Abort, and explicit Resume.
      - LocalWrite must be explicitly selected and ready.

4. Identity/authorization
      - Server remains authoritative.
      - Handles unauthorized, requester/approver separation, stale versions, decided approvals, and manual reconciliation using sanitized messages.
      - Requester identity is omitted because the existing safe preview API does not expose it.

5. Restart/resume
      - Authorized Waiting records are reconstructed from durable service state.
      - No approval data is stored in browser storage.
      - Initial CAS row version 0 is correctly supported.
      - Manual-reconciliation runs are displayed as non-resumable.
      - Fixed zero-based ServiceEventV2 cursor handling.

6. Activity integration
      - Added Approval required, Waiting for decision…, and Resuming….
      - Stop is hidden while the run is durably Waiting.
      - Success is shown only after authoritative terminal status.

7. Tests
      - Frontend: 23 passed.
      - Desktop Rust: 8 passed.
      - TypeScript, ESLint, formatting, strict workspace Clippy: passed.
      - Tauri debug build: passed.
      - Workspace tests excluding agent-mcp-adapters: passed.
      - Full workspace test exposed a pre-existing M11 failure: termination_reaps_process_group_descendant reproducibly reports a surviving descendant in the Tauri container. No MCP files were changed.

8. Screenshots
      - Not produced.

9. Deviations
      - No runtime approval, policy, audit, identity, persistence, or containment semantics changed.
      - Approval identity is not rendered because it is absent from the current safe service projection.
      - No commit created.

# M16.5 reviewing

• 1. Exact Approve/Deny command path
      - JavaScript invokes approval_submit_decision with only sessionId, runId, waitId, expectedRowVersion, and approve | deny.
      - Tauri command: apps/agent-desktop/src-tauri/src/lib.rs:133
      - Fixed service endpoint and strict body: apps/agent-desktop/src-tauri/src/service_client.rs:826
      - Tests verify exact approve/deny arguments and explicit resume: apps/agent-desktop/src/app/App.test.tsx:233

  1. ACL/security confirmation
      - Dedicated allow-approval-submit-decision permission.
      - No generic HTTP/path command.
      - No ActionDigest, ToolCallId, capsule, action arguments, or transport credentials exposed to JavaScript.
      - Rust request body uses deny_unknown_fields.
      - No implementation change was required.

  2. M11 baseline comparison
      - Compared current worktree with clean baseline f5678c44a6f763cabc61eb7cf8e6133aff7837ce.
      - MCP implementation, fixture, and test files are byte-identical.
      - The workspace run initially reproduced the failure on current, then passed unchanged. Subsequent results:
          - Isolated test: current 3/3, baseline 3/3.
          - Full stdio suite: current 12/12 repeated runs, baseline 12/12.
          - Full workspace rerun: current PASS, baseline PASS.

      - Process evidence showed fixture server and /bin/sleep 30 in the same process group, with no surviving process after successful cleanup.
      - Classification: pre-existing/environment-sensitive flaky test, not M16.5-induced. No MCP changes made.

  3. Final M16.5 verdict: PASS
      - Frontend: typecheck, ESLint, 23/23 tests, 5/5 approval tests, production build PASS.
      - Rust: formatting, strict workspace Clippy, desktop tests 8/8 PASS.
      - Tauri no-bundle build PASS.
      - Final full workspace test PASS.
      - No commit created.

# M16.6 Implementation reporting

• 1. Changed tree
      - Service/read boundaries: agent-identity, agent-harness, agent-persistence-sqlite, agent-service, agent-service-http
      - Desktop bridge and ACL: apps/agent-desktop/src-tauri
      - Desktop history UX: React contracts, controller, sidebar, inspector, and tests

  1. Service/history APIs
      - Added owner-filtered, bounded session pagination.
      - Added bounded run-history pagination per session.
      - Added metadata-only conversation and run projections.
      - SQLite remains hidden behind provider-neutral read ports.

  2. Sidebar/history UX
      - Replaced mock sidebar entries with durable service data.
      - Shows latest status, activity time, Waiting, failure, and reconciliation indicators.
      - Prior runs appear on demand in the inspector.
      - Titles are deterministic; rename/archive were not added.

  3. Restoration/reconnect flow
      - Reconnect loads summaries first, then selected-session run metadata.
      - Existing runs are polled from their durable cursor without creating another run.
      - Available volatile results are displayed.
      - Missing terminal results show Result unavailable; no regeneration occurs.

  4. Waiting integration
      - Durable Waiting sessions reappear through the existing M16.5 approval flow.
      - Approval state remains server-authoritative.
      - No approval or conversation payload is stored in browser storage.

  5. Performance strategy
      - Session history is bounded to four 64-item pages.
      - Run details and events load only for the selected conversation.
      - Inspector and Markdown rendering remain lazy-loaded.

  6. Tests
      - Frontend: typecheck, ESLint, production build, 26 tests passed.
      - Desktop Rust: 8 tests passed.
      - Targeted identity/persistence/service/HTTP tests passed.
      - Strict workspace Clippy and formatting passed.
      - Tauri no-bundle application build passed.
      - Workspace regression suite passes when excluding agent-mcp-adapters.

  7. Screenshots
      - Not captured.

  8. Deviations
      - Full workspace testing remains blocked by the pre-existing M11 termination_reaps_process_group_descendant failure. It reproduces in isolation in the established container; M16.6 does not modify MCP
        lifecycle code.

      - Terminal application results remain volatile across service restart by design.
      - No rename or archive operation was added.
      - No commit was created.

# M16.7 implementation reporting

• 1. Changed Tree

- Added apps/agent-desktop/src/components/SettingsView.tsx.
- Updated desktop app, strict bridge contracts, Tauri service client, service/HTTP identity projections, workflow budgets, and readiness composition.
- No commit created.

  1. Settings UX

- Lazy-loaded Settings from app bar and command palette.
- System/Light/Dark theme controls.
- Compact grouped rows, restrained status indicators, responsive wrapping, and keyboard focus states.

  1. Runtime/Operations Status

- Shows service, model, OCPP/Standards knowledge, workflows, LocalWrite, containment, identity, versions, limits, budgets, MCP readiness, and authorized reconciliation count.
- Added authenticated metadata-only /v1/runtime/status.

  1. Identity/Authorization

- Added default-deny ReadRuntimeStatus.
- Returns only bounded principal ID, principal kind, and current roles.
- Reconciliation counts are fetched only for principals carrying the Operator role.

  1. Security Boundary

- Existing service_readiness Tauri command aggregates the new projections.
- No new Tauri command, permission, or capability.
- Deployment fingerprint and git identity are removed in Rust before data reaches JavaScript.
- No configuration mutation, generic HTTP, paths, credentials, or runtime authority exposed.

  1. Bundle Impact

- Settings is a separate lazy chunk: 7.13 kB, 2.39 kB gzip.
- Existing 559 kB main-bundle warning remains unchanged.

  1. Tests

- Frontend: typecheck, ESLint, 31 tests, and production build passed.
- Desktop Rust: 9 tests, strict Clippy, and Tauri release build passed in the GTK/WebKit image.
- Full workspace strict Clippy passed.
- Workspace tests excluding M11 adapter passed.
- Full workspace reproduced only the documented M11 termination_reaps_process_group_descendant failure; no MCP files were changed.
- cargo fmt --all -- --check and git diff --check passed.
- Codebase Memory blast-radius scan found no unexpected authority-boundary change.

  1. Screenshots

- None captured; validation was headless without a running desktop service.

  1. Deviations

- Normal build output directory was owned by a previous container user, so the production frontend build used /tmp/ela-m167-dist-final.
- The known M11 process-reaping regression remains documented separately and unchanged.
