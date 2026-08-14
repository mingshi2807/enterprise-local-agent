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
