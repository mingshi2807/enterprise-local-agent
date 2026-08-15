09.08.2026

# M0

M0 establishes domain contracts and adapter ports only. It does not implement
the deterministic loop or invoke models or tools.

## Proposed workspace

```text
enterprise-local-agent/
├── Cargo.toml
├── Cargo.lock
├── apps/
│   └── agent-cli/
│       ├── Cargo.toml
│       └── src/main.rs
└── crates/
    ├── agent-core/
    │   ├── Cargo.toml
    │   └── src/
    │       ├── lib.rs
    │       ├── ids.rs
    │       ├── budget.rs
    │       ├── run.rs
    │       ├── capability.rs
    │       ├── model.rs
    │       ├── tool.rs
    │       └── event.rs
    └── agent-harness/
        ├── Cargo.toml
        └── src/
            ├── lib.rs
            ├── error.rs
            ├── lifecycle.rs
            ├── policy.rs
            ├── registry.rs
            └── ports/
                ├── mod.rs
                ├── model.rs
                ├── tool.rs
                └── audit.rs
```

The root workspace configuration should begin with:

```toml
[workspace]
members = [
    "apps/agent-cli",
    "crates/agent-core",
    "crates/agent-harness",
]
resolver = "3"

[workspace.package]
edition = "2024"
rust-version = "1.85"
```

Rust 1.85 is the minimum stable release supporting Edition 2024. Newer stable
toolchains may be used for development and CI.

## Crate responsibilities

### `agent-core`

`agent-core` contains the pure, provider-independent domain vocabulary:

- Validated identifier newtypes.
- Explicit run budgets and usage.
- Typed run lifecycle and terminal outcomes.
- Tool capability classification.
- Provider-neutral model and tool data.
- Serializable, metadata-only agent events.
- Domain validation errors.

It performs no I/O, has no asynchronous runtime dependency, and knows nothing
about the harness, providers, frameworks, or adapters.

### `agent-harness`

`agent-harness` is the application-policy boundary:

- Owns model, tool, and audit port traits.
- Enforces run lifecycle transitions and budgets.
- Owns the tool registry.
- Allows only `ReadOnly` tools in M0.
- Produces audit events.
- Prevents adapters from bypassing runtime policy.

M0 does not call `ModelPort` or execute `ToolPort`. The ports exist as contracts
for later milestones.

Ports belong in `agent-harness`, rather than `agent-core`, because they express
harness use cases. Keeping the core limited to domain concepts also prevents
future adapters from creating an alternate path around harness policy.

### `agent-cli`

`agent-cli` is a thin composition root:

- Initializes tracing.
- Constructs the M0 harness components.
- Reports foundation, version, or readiness information.
- Converts top-level failures into process exit behavior.

It does not implement a chat loop, provider configuration, network calls, or
operating-system command tools.

## Dependency direction

```text
agent-cli ───────► agent-harness ───────► agent-core
     └──────────────────────────────────► agent-core
```

Future model, tool, and audit adapters depend inward on `agent-harness` and
`agent-core`. Neither core nor harness depends on adapters.

## Minimal public domain types

The proposed M0 public surface is:

```text
Identifiers
  RunId
  SessionId
  UserId
  ToolCallId

Execution
  RunContext
  RunBudget
  BudgetUsage
  BudgetDimension
  RunStatus
  RunOutcome
  RunFailureKind

Capabilities
  CapabilityKind
    ReadOnly
    LocalWrite
    ExternalWrite
    Privileged

Model boundary data
  ModelMessage
  ModelRole
  ModelRequest
  ModelResponse
  ModelOutputPart
  TokenUsage

Tool boundary data
  ToolName
  ToolDefinition
  ToolCall
  ToolInput
  ToolResult
  ToolOutput

Audit
  AgentEvent
  AgentEventKind
  EventSequence
```

The run status should use a shape that cannot represent contradictory terminal
state:

```rust
enum RunStatus {
    Pending,
    Running,
    Finished(RunOutcome),
}
```

`RunBudget` contains finite, validated limits for:

- Maximum steps.
- Maximum model calls.
- Maximum tool calls.
- Maximum elapsed time.

Zero model and tool calls are valid for M0, but the step and elapsed-time limits
must be nonzero.

The following concepts are deferred until a milestone exercises them:
`CapabilitySet`, approvals, streaming, sandbox policy, retrieval, graph phases,
persistence records, and knowledge types.

## Minimal ports

The adapter ports should be object-safe, asynchronous, and runtime-independent:

```rust
pub trait ModelPort: Send + Sync {
    fn invoke<'a>(
        &'a self,
        request: ModelRequest,
    ) -> PortFuture<'a, Result<ModelResponse, ModelPortError>>;
}

pub trait ToolPort: Send + Sync {
    fn definition(&self) -> &ToolDefinition;

    fn invoke<'a>(
        &'a self,
        call: ToolCall,
    ) -> PortFuture<'a, Result<ToolResult, ToolPortError>>;
}

pub trait AuditSink: Send + Sync {
    fn record<'a>(
        &'a self,
        event: &'a AgentEvent,
    ) -> PortFuture<'a, Result<(), AuditPortError>>;
}
```

`PortFuture` should use `Pin<Box<dyn Future + Send>>`. This permits the traits to
be used behind `Arc<dyn ...>` without introducing `async-trait` or leaking Tokio
types.

Model and tool ports should not receive the entire `RunContext`. The harness
retains identity, budget, and policy data and passes adapters only the data they
require.

Tool inputs and outputs may use wrapped `serde_json::Value` at the dynamic
adapter boundary. Raw `Value` should not spread through public APIs.

## Error strategy

- `agent-core` uses focused `thiserror` validation errors for invalid budgets,
  illegal state transitions, and other domain invariant violations.
- `agent-harness` uses `thiserror` for policy denial, registry conflicts, budget
  exhaustion, and sanitized port failures.
- `agent-cli` uses `anyhow` only for application composition and user-facing
  context.
- Provider errors are mapped into stable error categories rather than exposed
  through public APIs.
- Persisted outcomes and audit events contain safe error kinds or codes, never
  opaque SDK errors or credential-bearing messages.

## Testing strategy

### `agent-core` tests

- Identifier uniqueness and serialization round trips.
- Budget validation and boundary exhaustion.
- Legal and illegal status transitions.
- Stable enum serialization names.
- Model and tool envelope serialization.

### `agent-harness` tests

- Object-safe fake ports compile.
- Only read-only tools can be registered or selected.
- Denied tools are never invoked.
- Budget accounting cannot exceed configured limits.
- Lifecycle events are ordered deterministically.
- Audit payloads exclude model prompts, outputs, and raw errors.

### `agent-cli` tests

- The startup or readiness command exits successfully.
- Composition errors produce a nonzero exit status.

All tests remain local and deterministic. Milestone verification requires:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

## Proposed dependencies

| Crate | Dependency | Purpose |
| --- | --- | --- |
| `agent-core` | `serde` with `derive` | Serializable domain state |
| `agent-core` | `serde_json` | Wrapped dynamic model and tool payloads |
| `agent-core` | `uuid` with `v4,serde` | Identifier newtypes |
| `agent-core` | `thiserror` | Domain validation errors |
| `agent-harness` | `agent-core` | Domain contracts |
| `agent-harness` | `thiserror` | Harness and port errors |
| `agent-harness` | `tracing` | Structured instrumentation |
| `agent-cli` | `agent-core`, `agent-harness` | Application composition |
| `agent-cli` | `anyhow` | Application-boundary errors |
| `agent-cli` | `tracing`, `tracing-subscriber` | Logging initialization |

Tokio, Clap, `async-trait`, HTTP clients, and time or database crates are not
needed in M0. Tokio should be introduced when the harness actually runs
asynchronous work, rather than merely because its ports return futures.

The following remain explicitly excluded from M0:

- Rig.
- graph-flow and petgraph.
- MCP.
- RAG and vector databases.
- HTTP servers and Tauri.
- Qwen and OpenAI provider code.
- Persistence and database code.

## Architecture risks

### Prematurely freezing model and tool schemas

Use private fields, constructors, and carefully selected `#[non_exhaustive]`
enums. M0 should not claim a stable 1.0 API.

### Harness growing into the loop engine

Restrict M0 to lifecycle, budgets, policy, registry, and ports. Observe,
Retrieve, Plan, Act, Verify, and Reflect belong to the future deterministic loop
crate.

### `serde_json::Value` becoming an untyped escape hatch

Contain it inside `ToolInput` and `ToolOutput` wrappers and validate it at
adapter boundaries.

### Audit events leaking prompts or secrets

Make M0 audit events metadata-only. Raw prompts, model output, provider errors,
and credentials must not be included by default.

### Ambiguous budget semantics

Define units, whether limits are inclusive, and which operation consumes each
counter before implementation.

### Audit failure policy

Fail-open versus fail-closed behavior must be an explicit later decision. M0
must not silently choose one.

### Serialization becoming accidental persistence compatibility

Use explicit Serde names and include an event schema version from the beginning.

---

# M0 review by web OpenAI

Your M0 architecture proposal is approved with the following required
adjustments.

1. RunContext belongs in agent-harness, not agent-core.

agent-core owns provider-independent domain values such as RunId, SessionId,
RunBudget, BudgetUsage, RunStatus and RunOutcome.

agent-harness owns the execution/application RunContext.

1. Separate ToolRegistry from authorization policy.

ToolRegistry is responsible only for registration, lookup, uniqueness and
tool metadata integrity.

It must not hard-code the temporary M0 ReadOnly policy.

Create a capability-policy abstraction in agent-harness and implement an M0
policy that allows ReadOnly and denies LocalWrite, ExternalWrite and
Privileged.

Authorization must occur outside ToolRegistry.

1. Remove max_steps from RunBudget in M0.

"Step" does not yet have defined semantics because the deterministic loop is
not implemented.

M0 RunBudget contains:

- max_model_calls
- max_tool_calls
- max_elapsed

max_model_calls and max_tool_calls may be zero.
max_elapsed must be finite and non-zero.

The future loop milestone will introduce max_iterations once LoopIteration has
a precise definition.

1. Define exact budget reservation semantics.

Before an operation begins, the harness checks whether usage is already at
the configured limit.

If usage >= limit, deny the operation without incrementing usage.

Otherwise reserve/increment the corresponding counter and allow the operation.

An operation must never execute first and then discover that it exceeded its
budget.

M0 does not need a Clock abstraction. Elapsed-time policy may accept an
externally supplied Duration for deterministic checking. Real deadline/clock
handling belongs to M1.

1. Keep this run-state representation:

RunStatus:

- Pending
- Running
- Finished(RunOutcome)

RunOutcome should distinguish at least:

- Completed
- Cancelled
- BudgetExceeded { dimension }
- Failed { kind }

Do not create contradictory terminal-state representations.

1. Defer UserId until the Identity design milestone.

Implement only:

- RunId
- SessionId
- ToolCallId

Do not prematurely define enterprise actor identity as a UUID user.

1. Keep the object-safe boxed-future port design.

Define a local PortFuture alias using:

Pin<Box<dyn Future<Output = T> + Send + 'a>>

Do not add async-trait.
Do not add Tokio.

ModelPort, ToolPort and AuditSink remain in agent-harness.

Ports must receive only the minimum provider-neutral request data required;
they must not receive RunContext.

1. Preserve the semantic distinction between:

- policy denial
- successful tool invocation returning a domain-level ToolResult
- ToolPort infrastructure/adapter failure

Policy denial occurs before ToolPort invocation.

ToolPortError represents executor/adapter failure, not normal tool-domain
results.

1. serde_json::Value is allowed only behind typed wrappers such as ToolInput,
ToolOutput and tool schema data.

Do not expose raw Value throughout the APIs.

1. Keep model contracts minimal and provider neutral.

ModelOutputPart must be capable of representing at least:

- textual output
- a provider-neutral ToolCall

Do not introduce Qwen-, OpenAI-, Rig- or other provider-specific fields.

Token usage values should be optional when a backend may not report them.

1. Security requirement:

Do not automatically derive Debug for payload-bearing types containing model
content or tool input/output.

This includes at least:

- ModelRequest
- ModelMessage
- ModelResponse where it contains content
- ToolInput
- ToolOutput
- ToolCall/ToolResult where raw payload is reachable

Either omit Debug or provide an explicitly redacted Debug implementation.

Never emit raw model/tool payloads through tracing or AgentEvent.

Metadata-only domain types may derive Debug normally.

1. AgentEvent remains metadata-only.

Use an event envelope conceptually containing:

- schema version
- RunId
- EventSequence
- typed AgentEventKind

Do not include raw prompt content, model output, raw tool input/output,
credentials, or provider error strings.

Do not add wall-clock timestamps to agent-core M0 events.
EventSequence provides deterministic ordering.

Infrastructure timestamps can be introduced later at the audit/persistence
boundary.

1. Do not overuse #[non_exhaustive].

Keep internal state-machine enums exhaustive so adding a new state forces
compiler errors in downstream exhaustive matches.

Use #[non_exhaustive] only where there is a concrete adapter/API extensibility
reason.

1. Do not declare rust-version = "1.85" unless we intend to CI-test and
maintain that MSRV.

For M0 use Rust Edition 2024 and resolver = "3".

Add rust-toolchain.toml using stable Rust with rustfmt and clippy.

1. Centralize third-party dependency versions under [workspace.dependencies]
and inherit them in member crates.

2. Do not add tracing to agent-harness in M0 unless implementation genuinely
requires it.

Semantic AgentEvents are sufficient for the M0 harness.

CLI may use tracing/tracing-subscriber for startup output.

1. Tests should validate our ID representation and serialization contract,
not statistically test UUID uniqueness.

Now update the proposed architecture mentally according to these decisions and
implement M0.

Create only:

apps/agent-cli
crates/agent-core
crates/agent-harness

Do not implement:

- Rig
- Tokio
- graph-flow
- petgraph
- MCP
- RAG
- vector DB
- network access
- Qwen/OpenAI providers
- persistence
- actual model invocation
- actual tool invocation

M0 must compile and demonstrate only the architectural foundation.

After implementation run:

cargo fmt --all -- --check

cargo clippy \
  --workspace \
  --all-targets \
  --all-features \
  -- -D warnings

cargo test --workspace --all-features

cargo tree --workspace

Also inspect all Cargo.toml files and verify the dependency direction is:

agent-cli -> agent-harness -> agent-core

with optional direct agent-cli -> agent-core.

There must be no reverse dependency.

Do not commit automatically.

At the end report:

1. final directory tree
2. public types implemented
3. public port traits implemented
4. budget semantics
5. capability-policy semantics
6. event/audit design
7. dependency graph
8. test results
9. clippy results
10. deliberately deferred functionality
11. any deviations from the approved architecture and why

---

09.08.2026

# M1 proposal

M1 should introduce one executable ExecutionHarness that owns adapter access and policy enforcement. RunContext remains the single mutable per-run owner; all mutating methods become crate-private so M2 cannot
  bypass the harness.

  No files were edited. The repository is clean at committed tag m0-foundation.

## 1. Proposed module and file changes

  crates/agent-core/src/
  └── event.rs                     # Extend metadata-only M1 event kinds

  crates/agent-harness/src/
  ├── lib.rs                       # Re-export M1 public surface
  ├── context.rs                   # Runtime deadline/cancellation state
  ├── execution.rs                 # ExecutionHarness orchestration
  ├── audit.rs                     # AuditFailurePolicy and audit helper
  ├── error.rs                     # Sanitized HarnessError model
  ├── fakes.rs                     # Deterministic fake adapters
  ├── policy.rs                    # Existing capability policy
  ├── ports.rs                     # Existing ports, now called operationally
  └── registry.rs                  # Existing registry

  crates/agent-harness/tests/
  └── execution_harness.rs         # Public-API integration tests

  apps/agent-cli/src/
  └── main.rs                      # Async fake-adapter demonstration

  No new crate is justified. The fake adapters are needed both by tests and by the M1 CLI demonstration, so keeping them in agent-harness::fakes is appropriate for now.

### RunContext change

  The current crates/agent-harness/src/context.rs:10 should gain private process-local runtime state:

  struct RunRuntime {
      started_at: tokio::time::Instant,
      deadline_at: tokio::time::Instant,
      cancellation: CancellationToken,
      audit_degraded: bool,
  }

  pub struct RunContext {
      // Existing IDs, budget, usage, status and sequence
      runtime: Option<RunRuntime>,
  }

  RunContext should stop implementing Clone, Serialize, and Deserialize. Cloning it would undermine single ownership, and deserializing a Running context without a valid monotonic deadline or cancellation domain
  would be unsafe.

  The component domain values remain serializable in agent-core. A persistence milestone can later define an explicit snapshot/resume contract.

## 2. Proposed public API

  pub struct HarnessConfig {
      pub audit_failure_policy: AuditFailurePolicy,
      pub audit_timeout: Duration,
  }

  pub struct ExecutionHarness {
      // Arc<dyn ModelPort>
      // ToolRegistry
      // Arc<dyn CapabilityPolicy>
      // Arc<dyn AuditSink>
      // HarnessConfig
  }

  impl ExecutionHarness {
      pub fn new(
          model: Arc<dyn ModelPort>,
          tools: ToolRegistry,
          capability_policy: Arc<dyn CapabilityPolicy>,
          audit: Arc<dyn AuditSink>,
          config: HarnessConfig,
      ) -> Result<Self, HarnessConfigError>;

      pub async fn start_run(
          &self,
          context: &mut RunContext,
      ) -> Result<CancellationToken, HarnessError>;

      pub async fn invoke_model(
          &self,
          context: &mut RunContext,
          request: ModelRequest,
      ) -> Result<ModelResponse, HarnessError>;

      pub async fn invoke_tool(
          &self,
          context: &mut RunContext,
          call: ToolCall,
      ) -> Result<ToolResult, HarnessError>;

      pub async fn complete_run(
          &self,
          context: &mut RunContext,
      ) -> Result<(), HarnessError>;

      pub async fn fail_run(
          &self,
          context: &mut RunContext,
          kind: RunFailureKind,
      ) -> Result<(), HarnessError>;

      pub async fn cancel_run(
          &self,
          context: &mut RunContext,
      ) -> Result<(), HarnessError>;
  }

  start_run returns a cloned CancellationToken. This allows another task to request cancellation while an invocation owns &mut RunContext.

  The existing public state-mutating methods—start, finish, budget reservation, and event allocation—should become pub(crate). Public callers retain read-only context accessors.

## 3. Lifecycle sequence

### Start

  Require Pending
    → calculate monotonic started_at/deadline
    → create cancellation token
    → allocate RunStarted event sequence
    → record audit
    → commit Running state and runtime data
    → return cloned CancellationToken

  Under FailClosed, start-audit failure leaves the run Pending. The attempted event sequence remains consumed.

### Complete

  Require Running
    → reject/terminalize pending cancellation or expired deadline
    → allocate Completed event
    → record audit
    → commit Finished(Completed)

  Under FailClosed, completion-audit failure leaves the run Running.

### Fail and cancel

  Failure, cancellation, and deadline expiry are safety terminalizations. They should commit terminal state even if the terminal audit fails:

- fail_run → Finished(Failed { kind })
- cancel_run → Finished(Cancelled)
- expired run deadline → Finished(BudgetExceeded { Elapsed })

  An audit failure is still returned or recorded as degraded, but it must not reopen a cancelled or expired run.

## 4. Model invocation sequence

  Require Running
    → check cancellation
    → check deadline
    → reserve model budget
    → allocate metadata invocation event
    → record pre-invocation audit
    → invoke ModelPort under cancellation/deadline race
    → allocate completion/failure event
    → record post-invocation audit
    → return ModelResponse or sanitized HarnessError

  Important details:

- Budget is reserved before audit and before ModelPort::invoke.
- If the budget is already exhausted, the port is never called.
- If pre-invocation audit fails under FailClosed, the port is not called.
- The reserved budget is not refunded after pre-audit failure; reservation already occurred according to the required ordering.
- Cancellation and deadline remain active while both audit and model futures are awaiting.
- Simultaneous cancellation/deadline readiness should prefer cancellation, matching the required check order.

  Conceptually:

  tokio::select! {
      biased;

      _ = cancellation.cancelled() => Err(HarnessError::Cancelled { ... }),
      _ = tokio::time::sleep_until(deadline) => {
          Err(HarnessError::DeadlineExceeded { ... })
      }
      result = model.invoke(request) => map_model_result(result),
  }

  Dropping the port future is cooperative local cancellation. A future real remote adapter may need idempotency/request IDs because dropping a future cannot guarantee that a remote service stopped work.

## 5. Tool invocation sequence

  Require Running
    → check cancellation
    → check deadline
    → registry lookup by ToolName
    → read ToolDefinition capability
    → CapabilityPolicy::authorize

  Denied branch:

  Allocate ToolDenied event
    → audit denial
    → return PolicyDenied

  The denied branch:

- Does not reserve tool budget.
- Does not call ToolPort.
- Returns PolicyDenied, unless a FailClosed denial-audit failure returns an audit error first.

  Allowed branch:

  Reserve tool budget
    → allocate invocation metadata event
    → record pre-invocation audit
    → invoke ToolPort under cancellation/deadline race
    → record completion/domain-failure/adapter-failure event
    → return ToolResult or sanitized HarnessError

  The semantic distinction remains:

  PolicyDenied
      Harness rejected the capability before invocation.

  Ok(ToolResult::DomainFailure { ... })
      Tool was invoked successfully and reported a normal domain failure.

  Err(HarnessError::ToolPort(...))
      Adapter/executor infrastructure failed.

## 6. Cancellation and deadline strategy

  Use tokio_util::sync::CancellationToken; no custom primitive is justified.

  A clone participates in the same cancellation domain. cancel() wakes tasks awaiting cancelled(). Child tokens are available later for graph branches: parent cancellation propagates downward, while child
  cancellation does not cancel the parent. M1 should use ordinary clones, not child tokens. Official CancellationToken documentation
  (<https://docs.rs/tokio-util/0.7.19/tokio_util/sync/struct.CancellationToken.html>)

### Deadline

- RunBudget.max_elapsed remains in agent-core.
- RunContext stores tokio::time::Instant values only after start.
- start_run computes deadline_at = started_at + max_elapsed.
- Overflow from checked_add becomes a sanitized start/configuration error.
- Each awaited audit/model/tool operation is bounded.

  No Clock trait is needed in M1. Tokio’s paused-time test support gives deterministic monotonic time:

  #[tokio::test(start_paused = true)]
  async fn expired_deadline_prevents_invocation() {
      // start run
      tokio::time::advance(max_elapsed).await;
      // invocation is denied
  }

  This requires Tokio’s test-util feature. Tokio paused-time documentation (<https://docs.rs/tokio/1.53.1/tokio/time/fn.pause.html>), time advancement (<https://docs.rs/tokio/1.53.1/tokio/time/fn.advance.html>)

### Audit timeout

  Audit writes also require an explicit bound. HarnessConfig.audit_timeout should be finite and nonzero.

  For normal operations, the effective audit deadline is the earlier of:

- The run deadline.
- now + audit_timeout.

  A cancellation/deadline terminal event may use the standalone audit timeout because the run deadline has already expired. This permits bounded final audit cleanup without allowing further model/tool execution.

## 7. Audit strategy

  pub enum AuditFailurePolicy {
      FailClosed,
      FailOpen,
  }

  The CLI uses FailClosed.

### FailClosed

  For pre-operation audit failure:

- Return HarnessError::Audit.
- Do not invoke the model/tool.
- Keep any budget already reserved.
- Keep the allocated event sequence consumed.

  For post-operation audit failure:

- The port has already executed and cannot be rolled back.
- Return an audit error marked AfterInvocation.
- Include a sanitized execution-effect marker indicating that the operation ran.
- Do not return the raw model/tool result.
- Callers must not blindly retry.

### FailOpen

- Mark the context’s audit state as degraded.
- Continue the guarded operation after pre-audit failure.
- Return the original model/tool result or port error after post-audit failure.
- Never include the sink error string in an AgentEvent.

### Sequence allocation

  RunContext remains the only sequence allocator:

  Read next sequence
    → checked increment in context
    → construct AgentEvent
    → await AuditSink::record

  The sequence advances before the sink is called. Therefore:

- Ordering is deterministic.
- Failed writes can create gaps.
- A sequence is never reused.
- Concurrent operations cannot interleave because each operation holds the single &mut RunContext.
- Wall-clock timestamps are not used for ordering.

  M1 should extend crates/agent-core/src/event.rs:87 with metadata-only invocation-completed and invocation-failed events. The event schema version should advance because the serialized event vocabulary changes.

  Events may include identifiers, capability class, budget counters, stable success/failure categories, and token counts. They must not contain raw prompts, responses, tool payloads, schemas, or provider error
  strings.

## 8. Error strategy

  Proposed sanitized top-level error:

  pub enum HarnessError {
      InvalidLifecycle {
          operation: HarnessOperation,
          status: RunStatus,
      },
      BudgetExceeded(BudgetExceeded),
      Cancelled {
          stage: ExecutionStage,
      },
      DeadlineExceeded {
          stage: ExecutionStage,
      },
      ToolNotFound {
          name: ToolName,
      },
      PolicyDenied(PolicyDenial),
      Audit {
          phase: AuditPhase,
          operation: HarnessOperation,
          effect: OperationEffect,
          kind: AuditPortError,
      },
      ModelPort(ModelPortError),
      ToolPort(ToolPortError),
      Context(RunContextError),
  }

  Supporting typed enums:

  enum ExecutionStage {
      Preflight,
      PreAudit,
      Invocation,
      PostAudit,
  }

  enum AuditPhase {
      BeforeInvocation,
      AfterInvocation,
      Lifecycle,
  }

  enum OperationEffect {
      NotStarted,
      Executed,
      StateCommitted,
  }

  This distinguishes retry-safe failures from failures after a side effect.

  Provider errors remain mapped into the existing stable ModelPortError and ToolPortError categories. Raw provider strings and SDK error types do not cross the port.

  ToolResult::DomainFailure remains an ordinary successful return from the harness.

## 9. Fake adapters and tests

### Fakes

  FakeModelPort:

- Scripted queue of ModelResponse or ModelPortError.
- Optional deterministic Tokio delay.
- Invocation counter.
- Optional shared trace for ordering assertions.

  FakeToolPort:

- Fixed validated ToolDefinition.
- Scripted ToolResult or ToolPortError.
- Invocation counter.
- Optional deterministic delay.

  InMemoryAuditSink:

- Stores cloned metadata events in insertion order.
- Exposes an immutable snapshot for assertions.
- Uses a short-lived standard mutex; no guard crosses an await.

  FailingAuditSink:

- Fails always or on a configured attempt number.
- Tracks attempted records.
- Supports deterministic pre- versus post-invocation failure tests.

  Required public-API integration tests:

- Model budget is reserved before port invocation.
- Zero model budget leaves model invocation count at zero.
- Allowed ReadOnly tool executes.
- Denied LocalWrite tool invocation count remains zero.
- Denied tool does not consume tool budget.
- Tool budget exhaustion prevents invocation.
- Cancellation prevents or interrupts operations.
- Expired deadline prevents invocation.
- Operations before start or after finish return lifecycle errors.
- Event sequences are deterministic.
- Failed audit attempts consume sequences.
- FailClosed pre-audit failure prevents invocation.
- FailOpen pre-audit failure allows invocation.
- Post-invocation audit failures expose OperationEffect::Executed.
- Provider failures map to stable sanitized errors.
- Tool domain failure remains Ok(ToolResult::DomainFailure).
- Serialized audit events contain none of the test prompt/input/output sentinel strings.

## 10. Dependency changes

  Centralize versions in root [workspace.dependencies].

### Tokio

  Current published Tokio is 1.53.1. Proposed features:

  tokio = {
      version = "1",
      default-features = false,
      features = ["rt", "macros", "time"]
  }

- rt: current-thread runtime used by CLI/tests.
- macros: #[tokio::main], #[tokio::test], and tokio::select!.
- time: monotonic deadlines, sleep, timeout handling.
- test-util: dev/test-only paused-time support.

  Do not enable rt-multi-thread, net, fs, process, signal, io-util, or full. Official Tokio feature list (<https://docs.rs/crate/tokio/1.53.1/features>)

### Tokio-util

  Current published tokio-util is 0.7.19:

  tokio-util = {
      version = "0.7",
      default-features = false,
      features = ["rt"]
  }

  tokio-util gates its sync module containing CancellationToken behind its rt feature; that feature enables the required Tokio runtime/synchronization support. Official tokio-util feature list
  (<https://docs.rs/crate/tokio-util/0.7.19/features>)

  No async-trait, futures crate, clock crate, network library, provider SDK, or additional synchronization dependency is needed.

## 11. Risks and tradeoffs

- &mut RunContext serializes operations per run. This is desirable for M1/M2 determinism.
- Future graph fan-out cannot share the context directly. The graph coordinator should reserve budgets and sequence numbers centrally, then pass owned operation tickets or immutable snapshots to branches
    before ordered fan-in.

- RunContext becoming process-local and non-serializable removes the M0 context round-trip test. This is intentional; persistence/resume requires an explicit snapshot contract.
- Fail-closed audit after invocation cannot undo the operation. Typed OperationEffect::Executed is essential to prevent unsafe retries.
- Pre-audit failure consumes budget because required ordering reserves first. Refunding would introduce ambiguous accounting.
- Failed audit attempts create sequence gaps. Reuse would be worse because it would destroy ordering evidence.
- Exposing CancellationToken couples the harness API to tokio-util, but avoids an unnecessary custom cancellation abstraction.
- Dropping a future cancels local work, not necessarily a remote side effect. Future adapters must document cancellation safety and idempotency.
- Fake adapters in the harness public surface are temporary milestone infrastructure; a separate test-support crate should wait until reuse justifies it.

  The dependency direction remains:

  agent-cli → agent-harness → agent-core

  agent-core remains independent of Tokio, tokio-util, providers, Rig, graph libraries, MCP, RAG, persistence, and I/O frameworks.

  This is the proposed M1 approval boundary; implementation remains paused.

# M1 review version by OpenAI web

The M1 architecture proposal is approved with the following required
adjustments.

Implement M1 after incorporating these decisions.

1. Do not expose tokio_util::sync::CancellationToken in the public harness API.

Use CancellationToken internally, but introduce an opaque harness-owned public
type such as:

RunCancellationHandle

It should expose only the application-level cancellation behavior required,
for example request_cancel() and optionally is_cancelled().

start_run returns RunCancellationHandle rather than CancellationToken.

The purpose is to prevent tokio-util from becoming part of our application
contract.

1. Lifecycle state is authoritative and must not be rolled back or reopened
because AuditSink failed.

In particular, complete_run must:

- require Running
- commit Finished(Completed)
- allocate/record the terminal event
- if audit fails, return a typed audit error with
  OperationEffect::StateCommitted
- keep the context Finished(Completed)

Never leave/revert the run to Running after completion has been committed.

The same invariant applies to all terminal states.

1. start_run has special fail-closed behavior.

Recommended sequence:

Pending
→ construct runtime state
→ commit Running
→ allocate RunStarted event
→ audit

If RunStarted audit fails under FailClosed:

- do not return a usable cancellation handle
- terminalize the same run as Finished(Failed { audit-unavailable kind })
- make a bounded best-effort attempt to audit that terminalization
- return the sanitized start/audit failure

Do not restore or leave the same RunId Pending for another start attempt.

A retry should create a new run/RunId.

If necessary, add an appropriate provider-independent RunFailureKind for audit
infrastructure unavailability.

1. Cancellation or deadline exhaustion must terminalize RunContext before an
operation returns.

If cancellation is observed before or during invoke_model/invoke_tool:

context.status must become Finished(Cancelled)

before returning the cancellation error.

If max_elapsed/deadline is exhausted:

context.status must become:

Finished(
    BudgetExceeded {
        dimension: Elapsed
    }
)

before returning.

Subsequent operations must therefore fail due to terminal lifecycle state.

Centralize this logic so preflight and in-flight cancellation/deadline paths
cannot diverge semantically.

Terminal audit failure must never undo or mask the primary safety
terminalization.

1. Clarify ToolRegistry ownership.

In M1, ToolRegistry may bind:

ToolName
→ ToolDefinition
→ Arc<dyn ToolPort>

Its responsibilities are:

- registration
- name uniqueness
- definition validation
- lookup/binding

It MUST NOT perform authorization.

invoke_tool ordering remains:

registry lookup
→ capability policy
→ budget
→ audit
→ ToolPort invocation

Do not create separate catalog/router crates or abstractions in M1.

1. Rename OperationEffect::Executed.

The harness cannot prove that a remote business side effect occurred merely
because a port future was invoked.

Use semantics such as:

OperationEffect:

- NotInvoked
- InvocationStarted
- StateCommitted

InvocationStarted means that the adapter invocation began and automatic retry
must not be assumed safe.

Do not claim remote execution certainty.

1. Introduce ModelCallId in agent-core now that real model invocation exists.

The harness generates a new ModelCallId for each model invocation.

Metadata audit events for model invocation start/completion/failure should
carry the same ModelCallId for correlation.

Tool calls continue to use ToolCallId.

Do not rely only on EventSequence adjacency for operation correlation.

1. Keep RunContext non-Clone and non-serializable.

Runtime fields such as:

- tokio::time::Instant
- deadline
- cancellation state
- audit degraded state

remain harness-local process state.

Do not introduce persistence snapshots yet.

1. Keep RunContext single-owner.

Guarded execution methods use &mut RunContext.

Do not introduce Arc<Mutex<RunContext>>.

Future graph fan-out will define its own branch/budget coordination model.

1. Keep the approved cancellation/deadline race semantics.

Using tokio::select! with biased ordering is acceptable.

Cancellation should be checked/polled before deadline so simultaneous
readiness deterministically prefers cancellation.

Document that dropping an adapter future is local cooperative cancellation and
does not prove that a remote side effect stopped.

Do not attempt to solve remote idempotency in M1.

1. Audit failure semantics:

FailClosed:

- pre-invocation audit failure prevents ModelPort/ToolPort invocation
- post-invocation audit failure reports
  OperationEffect::InvocationStarted
- lifecycle audit after committed state reports
  OperationEffect::StateCommitted

FailOpen:

- mark RunContext audit state degraded
- continue where safe
- preserve the original operation result/error

For cancellation/deadline/failure terminalization:

- the primary safety terminal outcome wins
- terminal audit failure must not reopen state
- mark audit degraded / bounded best-effort audit as appropriate

Do not include sink raw errors in AgentEvent.

1. Preserve current budget ordering for M1.

Budget reservation occurs before pre-invocation audit as already proposed.

Therefore a FailClosed pre-audit failure may consume the reserved invocation
slot.

Document that this is an execution reservation budget, not a provider billing
metric.

Do not implement refunds in M1.

Denied capability requests still consume no tool execution budget.

1. Fakes must not become unconditional production API.

Prefer:

agent-harness/src/testing.rs

with a Cargo feature such as:

test-support = []

and expose the module only under test or that feature.

The M1 CLI demonstration may temporarily enable test-support.

Do not create agent-test-support as a separate crate yet.

1. Keep the proposed fake adapters:

- FakeModelPort
- FakeToolPort
- InMemoryAuditSink
- FailingAuditSink

They should support deterministic invocation counts, scripted results and
ordering assertions.

1. Event model remains metadata-only.

Extend event kinds as necessary for:

- model invocation started/completed/failed
- tool invocation started/completed/domain-failed/adapter-failed
- tool policy denied
- lifecycle terminal events

Use ModelCallId/ToolCallId for correlation.

EventSequence remains deterministic ordering.

Do not add:

- raw prompts
- model output
- tool payloads
- provider errors
- credentials
- wall-clock timestamps

Advance the event schema version if required by the existing schema-version
contract.

1. Tokio dependencies remain confined outside agent-core.

Use only features actually required.

Tokio:

- rt
- macros
- time

test-util only where tests require paused time.

tokio-util:

- rt for CancellationToken support.

Do not enable full, net, fs, process, signal or unrelated features.

1. Keep errors sanitized.

Maintain explicit separation between:

- lifecycle error
- budget exhaustion
- cancellation
- deadline
- policy denial
- audit failure
- ModelPort failure
- ToolPort failure
- ToolResult domain failure

Raw SDK/provider/audit error strings must not cross the harness boundary.

1. Implement the deterministic CLI demonstration:

- create one RunContext
- start it
- invoke one FakeModelPort response
- invoke one allowed ReadOnly FakeToolPort
- complete the run
- report RunId, final status and budget usage

Do not display raw prompt/tool payloads as part of audit logging.

1. Add/maintain deterministic tests for at least:

- model budget reserved before invocation
- zero model budget never invokes ModelPort
- ReadOnly tool executes
- LocalWrite denied and ToolPort never called
- denied tool consumes no tool budget
- exhausted tool budget prevents invocation
- cancellation before invocation terminalizes the run
- cancellation during invocation terminalizes the run
- deadline before invocation terminalizes the run
- deadline during invocation terminalizes the run
- operations before start fail
- operations after terminal state fail
- complete_run remains Finished(Completed) if its audit fails
- start FailClosed audit failure leaves run terminal, not Pending/Running
- deterministic EventSequence
- ModelCallId correlates model start/completion events
- ToolCallId correlates tool events
- FailClosed pre-audit prevents adapter invocation
- FailOpen pre-audit permits invocation and marks audit degraded
- post-invocation audit failure reports InvocationStarted
- lifecycle audit failure reports StateCommitted
- terminal safety state is preserved if terminal audit fails
- provider errors remain sanitized
- ToolResult domain failure remains an ordinary successful port invocation
- serialized events contain none of the sentinel prompt/input/output/error
  strings

1. After implementation run:

cargo fmt --all -- --check

cargo clippy \
  --workspace \
  --all-targets \
  --all-features \
  -- -D warnings

cargo test --workspace --all-features

cargo tree --workspace

cargo tree -p agent-core

Also run:

git diff --check

Do not commit automatically.

At the end report:

1. final changed tree
2. ExecutionHarness API
3. RunCancellationHandle API
4. lifecycle semantics
5. model execution sequence
6. tool execution sequence
7. cancellation/deadline terminalization
8. audit failure semantics
9. ToolRegistry binding model
10. event correlation model
11. dependencies/features
12. tests and quality-gate results
13. dependency direction
14. deliberate deferrals
15. any deviations and justification

# M1 Patch

Perform one final semantic verification of M1 before commit.

Do not make broad architectural changes.

Inspect the implemented budget-exhaustion behavior for model and tool calls.

Hard execution-budget exhaustion must have these semantics:

1. If max_model_calls is already exhausted when invoke_model is requested:

   - ModelPort must not be invoked.
   - RunContext must terminalize as:
     Finished(BudgetExceeded { dimension: ModelCalls })
   - The budget-exhaustion terminal state must be committed before returning.
   - Any terminal audit failure must not reopen or mask this primary state.
   - Subsequent guarded operations must fail because the run is terminal.

2. If max_tool_calls is already exhausted when an authorized tool invocation is requested:

   - ToolPort must not be invoked.
   - RunContext must terminalize as:
     Finished(BudgetExceeded { dimension: ToolCalls })
   - Terminal audit failure must not reopen the run.
   - Subsequent guarded operations must fail because the run is terminal.

3. A capability-policy denial is NOT budget exhaustion:

   - It consumes no tool execution budget.
   - It does not terminalize the run solely because the capability was denied.

4. A FailClosed pre-invocation audit failure after a successful budget reservation is also NOT budget exhaustion:

   - The reserved slot remains consumed according to the approved M1 semantics.
   - The run does not become BudgetExceeded unless the next attempted reservation actually encounters the configured limit.

5. Verify ToolRegistry cannot create an inconsistent binding between ToolDefinition and ToolPort.
   Prefer registration to derive the definition from ToolPort::definition(), or otherwise explicitly validate equality/invariants before accepting the binding.

Add deterministic tests if these exact semantics are not already covered.

At minimum verify tests for:

- zero model budget terminalizes with ModelCalls and never invokes ModelPort
- exhausted model budget terminalizes and prevents later operations
- zero authorized-tool budget terminalizes with ToolCalls and never invokes ToolPort
- denied tool does not terminalize as budget exhausted
- terminal audit failure preserves ModelCalls/ToolCalls budget-exhaustion state
- ToolRegistry cannot register inconsistent definition/port metadata

Then run:

cargo fmt --all -- --check

cargo clippy
--workspace
--all-targets
--all-features
-- -D warnings

cargo test --workspace --all-features

git diff --check

Report whether any patch was required.

Do not commit automatically.

----
14.08.2026

## Propoal Codex

# M2 Architecture Proposal — Deterministic Loop Engineering

## Summary

  Create a new agent-loop crate containing a pure typed loop state machine and a sequential asynchronous driver. agent-core gains only shared loop vocabulary and budget/event metadata; agent-harness remains
  the sole lifecycle, budget, audit, cancellation, deadline, model, and tool membrane.

  Key decisions:

- LoopPhase lives in agent-core because it appears in shared audit events and client-visible runtime metadata.
- Legal transitions and LoopState live in agent-loop.
- One LoopProgram trait has six typed phase methods; this avoids six separate handler abstractions.
- Phase methods receive a narrow LoopEffects façade, never ports or audit sinks.
- The caller starts the run and retains RunCancellationHandle; LoopEngine owns loop-derived completion/failure.
- Iterations are reserved through ExecutionHarness::begin_iteration.
- Loop progression is recorded through a narrow harness-owned control-event API.
- Non-terminal harness errors fail M2 immediately. Only Reflect::Continue starts another iteration.
- M2 remains sequential with &mut RunContext.

## 1. Proposed tree/module changes

  Cargo.toml
  apps/agent-cli/
  └── src/main.rs                     # Replace M1 sequence with scripted M2 loop demo

  crates/agent-core/src/
  ├── budget.rs                       # max_iterations, usage, Iterations dimension
  ├── event.rs                        # loop metadata events, schema version 3
  ├── loop_control.rs                 # shared LoopPhase/decision/failure vocabulary
  ├── run.rs                          # loop-specific RunFailureKind variant
  └── lib.rs                          # exports

  crates/agent-harness/src/
  ├── context.rs                      # private iteration reservation
  ├── execution.rs                    # begin_iteration + loop-event recording
  ├── error.rs                        # new guarded operations/stages
  └── lib.rs                          # exports

  crates/agent-loop/
  ├── Cargo.toml
  └── src/
      ├── lib.rs
      ├── state.rs                    # pure legal-transition state machine
      ├── program.rs                  # LoopProgram, LoopEffects, verification
      ├── engine.rs                   # sequential asynchronous driver
      ├── error.rs                    # loop/program/transition errors
      └── tests.rs                    # deterministic M2 tests

  This preserves the M1 rule that all RunContext mutation remains crate-private inside the harness, as currently established in crates/agent-harness/src/context.rs:9.

## 2. Dependency direction

  agent-cli
     ├── agent-loop
     ├── agent-harness
     └── agent-core

  agent-loop
     ├── agent-harness
     └── agent-core

  agent-harness
     └── agent-core

  Forbidden directions:

- agent-core must not depend on agent-harness or agent-loop.
- agent-harness must not depend on agent-loop.
- agent-loop production code must not import or invoke ModelPort, ToolPort, or AuditSink.
- No provider, network, graph, retrieval, persistence, or orchestration dependency is added.

## 3. Public API

### Shared vocabulary in agent-core

  pub enum LoopPhase {
      Observe,
      Retrieve,
      Plan,
      Act,
      Verify,
      Reflect,
  }

  pub enum VerificationResult {
      Passed,
      Failed,
  }

  pub enum LoopFailureKind {
      Program { phase: LoopPhase },
      VerificationFailed,
      NoProgress,
  }

  pub enum ReflectDecision {
      Complete,
      Continue,
      Fail { kind: LoopFailureKind },
  }

  RunFailureKind gains:

  Loop { kind: LoopFailureKind }

  Budget APIs become:

  RunBudget::new(
      max_model_calls: u32,
      max_tool_calls: u32,
      max_iterations: u32,
      max_elapsed: Duration,
  )

  BudgetUsage::new(
      model_calls: u32,
      tool_calls: u32,
      iterations: u32,
  )

  BudgetDimension gains Iterations.

### Harness extensions

  impl ExecutionHarness {
      pub async fn begin_iteration(
          &self,
          context: &mut RunContext,
      ) -> Result<u32, HarnessError>;

      pub async fn record_loop_event(
          &self,
          context: &mut RunContext,
          event: LoopProgressEvent,
      ) -> Result<(), HarnessError>;
  }

  begin_iteration is the only public way to reserve an iteration. LoopProgressEvent excludes IterationStarted, preventing callers from forging reservation events.

### agent-loop API

  pub struct LoopEngine;

  impl LoopEngine {
      pub async fn run<P: LoopProgram>(
          &self,
          harness: &ExecutionHarness,
          context: &mut RunContext,
          program: &mut P,
          working_state: &mut P::WorkingState,
      ) -> Result<LoopRunSummary, LoopError>;
  }

  pub trait LoopProgram: Send {
      type WorkingState: Send;

      fn observe(... ) -> LoopFuture<'_, Result<(), LoopStepError>>;
      fn retrieve(...) -> LoopFuture<'_, Result<(), LoopStepError>>;
      fn plan(... ) -> LoopFuture<'_, Result<(), LoopStepError>>;
      fn act(... ) -> LoopFuture<'_, Result<(), LoopStepError>>;
      fn verify(...) -> LoopFuture<'_, Result<VerificationResult, LoopStepError>>;
      fn reflect(
          ...,
          verification: VerificationResult,
      ) -> LoopFuture<'_, Result<ReflectDecision, LoopStepError>>;
  }

  Each method receives:

- current iteration number;
- &mut WorkingState;
- a LoopEffects value exposing only invoke_model and invoke_tool.

  LoopFuture uses std::future::Future and Pin<Box<...>>; no async-trait or production Tokio dependency is needed.

## 4. State-machine design

  LoopState is a pure synchronous state machine in agent-loop:

  pub struct LoopState {
      current_iteration: Option<u32>,
      completed_iterations: u32,
      position: LoopPosition,
  }

  enum LoopPosition {
      Ready,
      PhaseReady(LoopPhase),
      PhaseRunning(LoopPhase),
      Finished(LoopTerminal),
  }

  Legal operations:

  1. begin_iteration(n) changes Ready → PhaseReady(Observe).
  2. enter_phase(expected) changes PhaseReady(expected) → PhaseRunning(expected).
  3. complete_phase(expected) advances exactly:
     Observe → Retrieve → Plan → Act → Verify → Reflect.

  4. Completing Reflect requires a typed ReflectDecision:
      - Continue completes the iteration and returns to Ready;
      - Complete completes the iteration and terminalizes loop control;
      - Fail completes the iteration and terminalizes with its stable kind.

  No API accepts a caller-selected next phase. Any mismatched phase or transition returns LoopTransitionError.

  Separating pure state from the asynchronous driver is preferred over embedding transitions directly in LoopEngine::run because it independently proves phase legality, exact ordering, and terminal behavior. A
  generic graph/state-machine framework would add no M2 value.

  LoopState may derive Serde for deterministic inspection and round-trip tests. This does not establish durable resume: it excludes RunContext, budgets’ runtime authority, audit state, cancellation, deadlines,
  and working data.

## 5. Phase execution design

  Use one LoopProgram trait with six methods, not six traits.

  LoopEffects contains private references to ExecutionHarness and RunContext and exposes only:

  pub async fn invoke_model(
      &mut self,
      request: ModelRequest,
  ) -> Result<ModelResponse, HarnessError>;

  pub async fn invoke_tool(
      &mut self,
      call: ToolCall,
  ) -> Result<ToolResult, HarnessError>;

  It contains no policy, budget, retry, audit, timeout, or transition logic; it only forwards to the existing harness methods in crates/agent-harness/src/execution.rs:73.

  Phase semantics:

- Observe: scripted deterministic update of working state.
- Retrieve: deterministic no-op or sets a typed retrieval_performed/empty-result state. No RetrievalPort.
- Plan: invokes one scripted fake model through LoopEffects; the response is data only.
- Act: invokes one scripted allowed ReadOnly tool through LoopEffects.
- Verify: returns VerificationResult::Passed or Failed.
- Reflect: returns ReflectDecision; it never accepts free-form text as a transition.

  Rejected alternatives:

- One execute_phase(LoopPhase) method allows phase/result mismatches that six typed methods prevent.
- A sans-I/O effect interpreter would enforce purity more strongly but prematurely introduces command/result protocol machinery.
- Putting all behavior directly in LoopEngine::run would make the engine both control runtime and application program.

  Program futures must not perform blocking or external work independently. External effects belong behind LoopEffects.

## 6. Iteration-budget design

  RunBudget and BudgetUsage gain max_iterations and iterations; zero is valid.

  ExecutionHarness::begin_iteration executes:

  1. Require RunStatus::Running.
  2. Run existing cancellation/deadline preflight.
  3. Call private RunContext::reserve_iteration.
  4. If exhausted, commit:
     Finished(BudgetExceeded { dimension: Iterations }).

  5. Emit IterationStarted { iteration, usage, limit }.
  6. Return the one-based iteration number.

  Reservation occurs immediately before LoopState enters Observe. No handler runs if reservation fails.

  Once incremented, usage is never refunded for:

- program failure;
- audit failure;
- model/tool failure;
- cancellation;
- elapsed deadline;
- normal Reflect failure.

  The current model/tool terminalization pattern at crates/agent-harness/src/execution.rs:81 is reused rather than reimplemented in agent-loop.

## 7. Lifecycle ownership

  Adopt split lifecycle ownership:

- The composition layer calls ExecutionHarness::start_run and retains RunCancellationHandle.
- LoopEngine::run requires an already Running context.
- LoopEngine owns loop-derived complete_run and fail_run calls.
- Fatal harness terminalization is preserved and never overwritten.

  This combines the useful parts of the two proposed options:

- Caller-owned start provides the cancellation handle before the loop future completes, without spawning tasks or adding Arc<Mutex<RunContext>>.
- Engine-owned completion/failure ensures every normal Reflect terminal decision is converted into exactly one terminal RunOutcome.
- CLI/API callers do not duplicate Reflect-to-lifecycle mapping.

  Passing Pending or terminal context to run returns a typed harness lifecycle error without executing a phase.

## 8. Working-state design

  LoopProgram owns an associated WorkingState. LoopEngine treats it as opaque and only passes &mut references to the active phase.

  The scripted M2 state may contain typed fields such as:

  struct ScriptedWorkingState {
      observed: bool,
      retrieval: ScriptedRetrieval,
      plan_response: Option<ModelResponse>,
      action_result: Option<ToolResult>,
  }

  Rules:

- No serde_json::Value property bag.
- Payload-bearing state is not added to LoopState or audit events.
- Avoid derived Debug where it could expose model/tool payloads.
- Provider-neutral ModelResponse and ToolResult may exist in working state because they are already core boundary types.
- Later M3/M8 programs can replace the associated state type without changing loop transitions or LoopEngine.

  Working state is not automatically serializable and is not a durable checkpoint contract.

## 9. Harness interaction, recovery, budgets, and cancellation

  Every phase boundary is recorded through ExecutionHarness::record_loop_event, whose preflight checks cancellation and elapsed deadline. This detects cancellation between phases without exposing
  CancellationToken.

  Long-running model/tool effects remain protected by the existing biased cancellation-first tokio::select! logic at crates/agent-harness/src/execution.rs:280.

  Recovery policy:

- No automatic retries.
- ToolResult::DomainFailure remains normal typed data and may lead Verify/Reflect to Continue or Fail.
- A non-terminal HarnessError immediately fails M2; it is not silently passed to Reflect.
- A terminal harness error stops immediately and preserves the existing RunStatus.
- Reflect::Continue is the only normal recovery path and begins a complete new iteration.

  Hard budgets:

   Dimension     Reservation/check                     Terminal outcome
  ━━━━━━━━━━━━  ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━  ━━━━━━━━━━━━━━━━━━━━━━━━━━━━
   ModelCalls    invoke_model                          BudgetExceeded(ModelCalls)
  ────────────  ────────────────────────────────────  ────────────────────────────
   ToolCalls     authorized invoke_tool                BudgetExceeded(ToolCalls)
  ────────────  ────────────────────────────────────  ────────────────────────────
   Iterations    begin_iteration                       BudgetExceeded(Iterations)
  ────────────  ────────────────────────────────────  ────────────────────────────
   Elapsed       harness preflight/invocation/audit    BudgetExceeded(Elapsed)

  No phase or iteration continues after any terminal outcome.

## 10. Observability design

  Add shared metadata-only loop events to AgentEventKind:

  AgentEventKind::Loop(LoopEventKind)

  LoopEventKind contains:

- IterationStarted { iteration, usage, limit }
- PhaseEntered { iteration, phase }
- PhaseCompleted { iteration, phase }
- ReflectDecision { iteration, decision }
- IterationCompleted { iteration }
- LoopCompleted { completed_iterations }
- LoopFailed { iteration, kind }

  Expected successful ordering:

  RunStarted
  IterationStarted(1)
  PhaseEntered(Observe)
  PhaseCompleted(Observe)
  ...
  PhaseEntered(Plan)
  ModelInvocationStarted
  ModelInvocationCompleted
  PhaseCompleted(Plan)
  PhaseEntered(Act)
  ToolInvocationStarted
  ToolInvocationCompleted
  PhaseCompleted(Act)
  ...
  PhaseCompleted(Reflect)
  ReflectDecision(Complete)
  IterationCompleted(1)
  LoopCompleted
  RunFinished(Completed)

  All loop events are created by RunContext::next_event, preserving the authoritative EventSequence allocation currently defined at crates/agent-harness/src/context.rs:137.

  Event fields contain only typed phases, decisions, stable failure categories, iteration numbers, and limits. They contain no prompts, outputs, tool payloads, schemas, provider strings, credentials, or
  timestamps.

  CURRENT_EVENT_SCHEMA_VERSION advances from 2 to 3 because serialized event vocabulary changes. Existing v2 variants remain readable; all new writers emit v3. No persistence migration is introduced.

  If a fatal harness operation already terminalized the run, no post-terminal loop event is attempted; the authoritative RunFinished event remains final evidence.

## 11. Error semantics

   Condition                    Returned error/control                    Run outcome
  ━━━━━━━━━━━━━━━━━━━━━━━━━━━  ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━  ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
   Illegal transition           LoopError::Transition                     Internal if encountered during an active run
  ───────────────────────────  ────────────────────────────────────────  ──────────────────────────────────────────────
   Iteration exhaustion         LoopError::Harness(BudgetExceeded)        BudgetExceeded(Iterations)
  ───────────────────────────  ────────────────────────────────────────  ──────────────────────────────────────────────
   Program failure              LoopError::Program { phase }              Failed(Loop::Program { phase })
  ───────────────────────────  ────────────────────────────────────────  ──────────────────────────────────────────────
   Harness lifecycle failure    LoopError::Harness(InvalidLifecycle)      Existing status preserved
  ───────────────────────────  ────────────────────────────────────────  ──────────────────────────────────────────────
   Cancellation                 LoopError::Harness(Cancelled)             Cancelled
  ───────────────────────────  ────────────────────────────────────────  ──────────────────────────────────────────────
   Deadline                     LoopError::Harness(DeadlineExceeded)      BudgetExceeded(Elapsed)
  ───────────────────────────  ────────────────────────────────────────  ──────────────────────────────────────────────
   Model adapter error          typed HarnessError::ModelPort             immediate Failed(Model) if still running
  ───────────────────────────  ────────────────────────────────────────  ──────────────────────────────────────────────
   Tool/policy/lookup error     typed tool-related HarnessError           immediate Failed(Tool) if still running
  ───────────────────────────  ────────────────────────────────────────  ──────────────────────────────────────────────
   Audit error                  typed HarnessError::Audit                 Failed(AuditUnavailable) if still running
  ───────────────────────────  ────────────────────────────────────────  ──────────────────────────────────────────────
   Reflect Fail                 normal LoopRunSummary, not exceptional    Failed(Loop { kind })

  If lifecycle finalization itself returns an audit error after committing state, return LoopError::Finalization containing the primary and finalization errors while preserving the committed terminal outcome.

  Terminal loop decisions take precedence over failures encountered while auditing their final metadata; an audit error must not reopen or replace the already selected primary outcome.

## 12. Deterministic test plan

### Pure state tests

- Exact legal Observe → Retrieve → Plan → Act → Verify → Reflect order.
- Wrong phase entry/completion is rejected.
- A phase cannot be entered twice or skipped.
- Complete, Continue, and Fail are accepted only after Reflect.
- Continue returns to iteration-ready state.
- Serialized LoopState round-trips without runtime fields.

### Loop/harness integration tests

- One full iteration completes.
- Continue produces exactly two complete traversals and two reservations.
- Complete starts no additional iteration.
- Reflect Fail terminalizes with its stable loop failure kind.
- Zero iteration budget executes no handler.
- Exhausted iteration budget terminalizes as Iterations.
- Mid-iteration program failure leaves iteration usage consumed.
- Cancellation/deadline mid-iteration leave usage consumed.
- One fake model invocation occurs through harness during Plan.
- One fake allowed ReadOnly tool invocation occurs through harness during Act.
- ModelCalls and ToolCalls exhaustion stop immediately.
- Elapsed deadline and cancellation stop immediately.
- No phase runs after terminalization.
- Tool domain failure remains typed and can drive Reflect Continue/Fail.
- Non-terminal provider/tool/audit errors fail without retry.

### Observability and boundary tests

- Exact loop/model/tool/audit EventSequence ordering.
- Failed audit attempts retain sequence-gap semantics.
- No prompt/output/tool sentinel appears in serialized events.
- Production agent-loop sources and public API contain no ModelPort, ToolPort, AuditSink, Tokio, provider, network, graph, or retrieval dependency.
- cargo tree -p agent-loop confirms only approved dependencies.

### Required gates

  cargo fmt --all -- --check
  cargo clippy --workspace --all-targets --all-features -- -D warnings
  cargo test --workspace --all-features
  cargo tree --workspace
  cargo tree -p agent-core
  cargo tree -p agent-loop
  git diff --check

## 13. CLI demonstration

  The CLI constructs:

- budget: one model call, one tool call, one iteration, bounded elapsed time;
- current fake model and fake ReadOnly tool;
- current in-memory audit sink;
- scripted working state and ScriptedLoopProgram.

  Execution:

  start_run
  LoopEngine::run
    iteration 1
    Observe
    Retrieve (scripted empty)
    Plan → fake model through LoopEffects/ExecutionHarness
    Act → fake ReadOnly tool through LoopEffects/ExecutionHarness
    Verify(Passed)
    Reflect(Complete)
    complete_run

- RunId
- iteration and phase progression derived from safe loop events
- final RunStatus
- model/tool/iteration usage
- audit_degraded

  The cancellation handle is retained by the CLI composition layer but does not require a spawned task. No prompt, model output, tool input/output, or audit payload is printed.

## 14. Dependency changes

  agent-loop production dependencies:

- agent-core: shared domain and loop vocabulary.
- agent-harness: guarded execution façade.
- serde: LoopState serialization.
- thiserror: typed loop errors.
- Standard library futures/pinning.

  Development-only dependencies:

- tokio with test-util for deterministic async cancellation/deadline tests.
- serde_json for state/event serialization assertions.
- agent-harness test-support feature for existing fake infrastructure.

  Workspace/CLI changes:

- Add agent-loop as a workspace dependency/member.
- Add agent-loop to agent-cli.

  No new third-party crate is introduced, and agent-loop has no direct production Tokio dependency.

## 15. Risks and tradeoffs

- Premature working-memory schema: mitigated by the associated opaque WorkingState; only loop control vocabulary is stabilized.
- Loop becoming a second harness: mitigated by keeping LoopEffects as forwarding-only and all policy/budget/audit/cancellation logic in ExecutionHarness.
- Handler bypass: production agent-loop exposes no ports; dependency/source checks guard the boundary.
- Core coupling: agent-core receives only client-visible phase/decision/failure/event vocabulary. Transition rules stay in agent-loop.
- Async handler misuse: program contract forbids blocking/external work outside LoopEffects; M2 implementations are scripted and deterministic.
- Graph composition: future graphs may treat one complete deterministic loop as a node; M2 adds no graph edge, branch, or fan-in semantics.
- Durable checkpoints: serializable LoopState is inspectable control metadata, not a resumable execution snapshot.
- Parallel branches: &mut RunContext intentionally serializes M2; future graph work must design owned branch state and deterministic budget/event fan-in.
- Rig structured outputs: future adapters may map structured responses into typed working state or Reflect decisions, but raw provider output never controls phase transitions.
- API compatibility: adding required iteration fields changes the current pre-1.0 constructors and serialized budget shape deliberately; no persistence compatibility promise is created in M2.

# M2 Web OpenAI review

The M2 architecture proposal is approved with the following required
adjustments.

Implement M2 after applying these decisions.

1. Keep agent-core lean.

LoopPhase belongs in agent-core because it is shared client/audit/runtime
metadata.

LoopFailureKind may also live in agent-core where required by RunFailureKind
and metadata events.

However keep orchestration-only types in agent-loop where possible.

In particular, VerificationResult belongs in agent-loop.

ReflectDecision should also remain in agent-loop unless there is a concrete
shared-domain requirement. If audit needs a decision representation, introduce
a minimal metadata-facing LoopDecisionKind in agent-core rather than moving the
entire control type into core.

Do not move LoopState/LoopPosition/LoopTerminal into agent-core.

1. Remove speculative NoProgress semantics.

Do not add LoopFailureKind::NoProgress in M2 because no no-progress algorithm
exists yet.

Only introduce failure kinds whose semantics are exercised in M2.

1. Do not serialize LoopState in M2.

LoopState is pure control state but is NOT yet a persistence or checkpoint
contract.

Do not derive Serialize/Deserialize merely for inspection.

Prefer Debug/Clone/PartialEq/Eq where appropriate.

Therefore agent-loop should not require a production serde dependency solely
for LoopState.

A future persistence/checkpoint milestone will define an explicit versioned
LoopSnapshot.

1. Add a narrow ExecutionHarness control checkpoint API.

Do not make loop cancellation/deadline detection depend solely on recording
an audit event.

Add a harness method conceptually like:

ExecutionHarness::checkpoint(&mut RunContext)

Its responsibility is only to:

- require Running
- check cancellation
- check elapsed deadline
- terminalize cancellation/deadline using existing M1 semantics

It must not consume model/tool/iteration budgets.

It must not invoke model/tool ports.

LoopEngine should call a harness checkpoint at deterministic phase boundaries.

record_loop_progress may also defensively preflight, but observability must not
be the only safety-control mechanism.

1. Every phase transition must verify the authoritative RunContext status.

After each LoopProgram phase future returns and before PhaseCompleted or the
next transition:

- inspect RunContext status

If the context is terminal for any reason:

- stop immediately
- preserve the existing terminal RunOutcome
- do not emit further normal phase progression events
- do not enter another phase

This invariant applies even if a LoopProgram implementation accidentally
ignored/swallowed a HarnessError returned by LoopEffects.

The loop must never continue after RunContext is terminal.

1. LoopProgram is trusted in-process application logic.

Document this accurately.

The M2 architectural contract requires model/tool/enterprise effects to use
LoopEffects, and the agent-loop production crate must contain no provider,
network, MCP, retrieval or direct port dependencies.

However do not claim the Rust type system prevents arbitrary downstream trusted
LoopProgram implementations from performing their own I/O.

Untrusted plugin execution and sandbox enforcement are outside M2 and belong to
future sandbox/plugin capability work.

1. Keep LoopEffects narrow.

LoopEffects may expose only guarded capabilities needed by M2, initially:

invoke_model(...)
invoke_tool(...)

It must not expose:

- ModelPort
- ToolPort
- AuditSink
- ToolRegistry
- budget mutation
- capability policy
- CancellationToken
- runtime deadlines
- direct RunContext mutation

All effects forward through ExecutionHarness.

1. Illegal loop transitions are fatal engine invariant failures.

If an illegal LoopState transition occurs while RunContext is still Running:

- terminalize the run using an appropriate stable internal/loop-invariant
  RunFailureKind
- return LoopError::Transition

Never return an internal transition error while leaving the run Running.

Do not panic.

1. Clarify terminal decision authority.

ReflectDecision::Complete or ReflectDecision::Fail is loop-control intent until
the corresponding ExecutionHarness lifecycle operation commits RunStatus.

Normal successful terminal sequence should conceptually be:

Reflect returns decision
→ pure LoopState accepts decision
→ record ReflectDecision metadata
→ record IterationCompleted metadata
→ record LoopCompleted/LoopFailed metadata
→ ExecutionHarness::complete_run/fail_run
→ authoritative Finished(...) RunStatus

Under FailClosed, if loop-metadata auditing fails while RunContext is still
Running, apply the normal M2 non-terminal harness-error failure policy.

Once ExecutionHarness has committed a terminal RunStatus, no audit/finalization
error may reopen or replace that state.

Preserve M1's rule that committed RunStatus is authoritative.

1. Do not duplicate M1 terminalization rules inside agent-loop.

After a HarnessError, inspect RunContext.status().

If RunContext is already Finished(...):

- preserve that terminal state
- stop immediately

If it is still Running:

- M2 uses the simple safe policy of failing the run through
  ExecutionHarness::fail_run with an appropriate stable failure category

Do not reimplement cancellation, deadline or budget terminalization in
agent-loop.

1. Iteration budgeting remains owned by ExecutionHarness.

Implement:

ExecutionHarness::begin_iteration(...)

The loop may not mutate BudgetUsage directly.

begin_iteration must:

- require Running
- check cancellation/deadline
- reserve iteration
- terminalize BudgetExceeded { Iterations } when exhausted
- emit metadata IterationStarted on successful reservation
- return the one-based iteration number

A started/reserved iteration is never refunded.

Zero max_iterations means no phase runs.

1. Add explicit iteration/audit tests.

FailClosed IterationStarted audit failure:

- iteration slot remains consumed
- Observe never runs
- LoopEngine sees a non-terminal audit error
- active run is terminalized according to M2 audit failure policy

FailOpen IterationStarted audit failure:

- iteration remains consumed
- audit_degraded is true
- Observe and the normal loop may continue

1. Preserve the pure synchronous LoopState design.

Use an explicit typed transition state machine.

No caller-selectable next phase.

Legal normal ordering is exactly:

Observe
→ Retrieve
→ Plan
→ Act
→ Verify
→ Reflect

Only Reflect may normally produce:

- Continue
- Complete
- Fail

Continue returns the pure loop state to iteration-ready.

Complete and Fail terminalize the pure loop-control state.

1. Keep one LoopProgram trait rather than six phase traits.

Use six typed methods on the one program abstraction.

Do not introduce a generic graph/state-machine framework.

Do not add a sans-I/O command protocol in M2 unless implementation reveals a
concrete blocker that cannot be solved cleanly with the approved LoopProgram /
LoopEffects design.

1. Keep working state opaque and program-owned.

LoopProgram::WorkingState remains an associated type.

LoopEngine does not inspect model/tool payload content.

Do not introduce:

- serde_json property bags
- persistence snapshots
- provider objects
- graph state
- retrieval objects

The scripted M2 WorkingState may use typed provider-neutral ModelResponse and
ToolResult values internally.

Avoid Debug on payload-bearing state where it could leak model/tool contents.

1. Retrieve remains deterministic/no-op in M2.

Do not add RetrievalPort, RAG, embeddings, vector DB or OVH Knowledge.

1. Recovery remains simple.

No automatic retries.

Reflect::Continue is the only normal loop recovery/repetition path.

ToolResult domain failures may be represented in WorkingState and influence
Verify/Reflect.

Non-terminal HarnessError causes the M2 run to fail.

Terminal HarnessError preserves the existing terminal RunStatus.

1. Loop observability remains metadata-only.

Add:

- IterationStarted
- PhaseEntered
- PhaseCompleted
- ReflectDecision metadata
- IterationCompleted
- LoopCompleted
- LoopFailed

Do not include:

- prompts
- model output
- tool input/output
- schemas
- provider/audit raw error strings
- credentials
- wall-clock ordering

EventSequence remains authoritative.

Use ModelCallId and ToolCallId for effect correlation as already established.

Advance CURRENT_EVENT_SCHEMA_VERSION to 3 if required by the event vocabulary
change, but do not introduce persistence migration or make backward-
compatibility guarantees yet.

1. LoopRunSummary must be metadata-only.

It may contain information such as:

- completed/reserved iteration counts
- terminal loop decision/category
- final status snapshot if appropriate

It must not expose raw working-state payloads.

1. M2 remains sequential.

Use &mut RunContext.

Do not introduce Arc<Mutex<RunContext>>, spawning, fan-out, child contexts or
parallel phases.

1. Agent-loop should have minimal production dependencies.

Expected production dependencies should preferably be only:

- agent-core
- agent-harness
- thiserror if required

No production Tokio dependency is expected.

Tokio/test-util may be dev-only for deterministic async tests.

Do not add serde to agent-loop solely to serialize LoopState.

1. Required deterministic tests include all tests from the proposal plus:

- LoopProgram swallowing a terminal HarnessError cannot make the loop advance
  after RunContext becomes Finished
- FailClosed IterationStarted audit failure consumes iteration but executes no
  Observe
- FailOpen IterationStarted audit failure marks degraded and permits Observe
- internal transition failure terminalizes an active run
- no loop metadata is emitted after an externally terminalized RunContext
- LoopState itself has no persistence/serialization contract in M2

1. CLI demonstration:

start_run
→ LoopEngine::run
→ one reserved iteration
→ Observe
→ Retrieve
→ Plan with one fake model call through LoopEffects/ExecutionHarness
→ Act with one allowed fake ReadOnly tool through LoopEffects/ExecutionHarness
→ Verify(Passed)
→ Reflect(Complete)
→ complete_run

Print only safe metadata:

- RunId
- loop phase/iteration progression derived from metadata events
- final RunStatus
- ModelCalls usage
- ToolCalls usage
- Iterations usage
- audit_degraded

Do not print raw prompt, model response, tool input/output or fake payloads.

1. After implementation run:

cargo fmt --all -- --check

cargo clippy \
  --workspace \
  --all-targets \
  --all-features \
  -- -D warnings

cargo test --workspace --all-features

cargo tree --workspace
cargo tree -p agent-core
cargo tree -p agent-harness
cargo tree -p agent-loop

git diff --check

Do not commit automatically.

At the end report:

1. final changed tree
2. agent-loop public API
3. LoopState transition model
4. LoopProgram and LoopEffects design
5. iteration-budget implementation
6. lifecycle ownership
7. checkpoint/cancellation/deadline behavior
8. harness-error handling
9. observability/event ordering
10. terminal decision semantics
11. WorkingState design
12. dependency tree
13. test and quality-gate results
14. deliberate deferrals
15. deviations and justification

15.08.2026

# M3

proposal codex waiting web OpenAI review approval :

# M3 Architecture Proposal

## 1. Inspected Rig version/API

  The inspected release is exactly rig-core 0.41.0, rig 0.41.0, and rig-agent 0.41.0, release commit 68b4eab, published July 28, 2026. The release introduced the rig-core/rig-agent split. Rig 0.41.0 release
  (<https://github.com/0xplaygrounds/rig/releases/tag/v0.41.0>).

  Adapter-relevant contracts reside in rig-core:

- CompletionModel
- CompletionRequest
- CompletionResponse<T>
- CompletionError
- Message, UserContent, AssistantContent, Text
- Usage
- OneOrMany

  CompletionModel has associated Response, StreamingResponse, and Client types and returns impl Future; it is explicitly not dyn-compatible. CompletionModel 0.41.0
  (<https://docs.rs/rig-core/0.41.0/rig_core/completion/request/trait.CompletionModel.html>).

  CompletionRequest carries ordered, non-empty chat_history, while CompletionResponse contains ordered assistant content, usage, raw response, and optional provider message ID. Exact 0.41.0 completion source
  (<https://github.com/0xPlaygrounds/rig/blob/68b4eabb8c9cf749ca73c917b9306e97fb0eda24/crates/rig-core/src/completion/request.rs>).

  No Agent, AgentRun, AgentRunner, ToolSet, memory, or retry hooks are required for one completion invocation.

## 2. Crate/dependency selection

  Use rig-core directly:

  rig-core = { version = "=0.41.0", default-features = false }

  Do not use the rig facade. Its defaults enable:

  rig-core/default
  agent
  derive
  rustls

  The agent feature pulls rig-agent, introducing the competing orchestration authority M3 excludes. Rig facade manifest
  (<https://github.com/0xPlaygrounds/rig/blob/68b4eabb8c9cf749ca73c917b9306e97fb0eda24/Cargo.toml>).

  Direct rig-core still has non-optional base reqwest and Tokio dependencies even with defaults disabled. M3 will neither construct an HTTP client nor perform network access; isolating this transitive surface in
  the provider crate limits its architectural reach. rig-core manifest (<https://github.com/0xPlaygrounds/rig/blob/68b4eabb8c9cf749ca73c917b9306e97fb0eda24/crates/rig-core/Cargo.toml>).

## 3. Proposed tree

  Cargo.toml
  Cargo.lock

  apps/
    agent-cli/
      Cargo.toml
      src/main.rs

  crates/
    agent-provider-rig/
      Cargo.toml
      src/
        lib.rs
        adapter.rs
        convert.rs
        testing.rs          # feature-gated test support
      tests/
        adapter.rs
        harness.rs
        loop_integration.rs
        boundaries.rs

  No production changes are expected in agent-core, agent-harness, or agent-loop.

  Cargo.lock will be updated normally during implementation, not deleted or regenerated independently.

## 4. Dependency direction

  agent-cli
    ├── agent-loop
    │     └── agent-harness
    │           └── agent-core
    │
    └── agent-provider-rig
          ├── agent-harness
          ├── agent-core
          └── rig-core

  Forbidden edges remain:

  agent-core    -/-> Rig
  agent-harness -/-> Rig
  agent-loop    -/-> Rig

  This preserves the existing provider-erasure boundary at crates/agent-harness/src/ports.rs:8 and the harness-only effect route in crates/agent-loop/src/program.rs:70.

## 5. RigModelAdapter public/internal API

  Minimal public API:

  pub struct RigModelAdapter<M> {
      model: M,
  }

  impl<M> RigModelAdapter<M> {
      pub const fn new(model: M) -> Self;
  }

  Implementation shape:

  impl<M> ModelPort for RigModelAdapter<M>
  where
      M: CompletionModel + Send + Sync + 'static,
  {
      fn invoke<'a>(
          &'a self,
          request: ModelRequest,
      ) -> PortFuture<'a, Result<ModelResponse, ModelPortError>>;
  }

  Rig’s existing trait bounds already constrain response and streaming-response types.

  The adapter remains generic because CompletionModel is not dyn-compatible. Type erasure occurs only at the existing application boundary:

  let model_port: Arc<dyn ModelPort> =
      Arc::new(RigModelAdapter::new(fake_rig_model));

  RigModelAdapter will not derive Debug, and it will not expose its model through payload-bearing inspection methods.

  Internal convert functions translate requests, responses, usage, and errors. No new public error type is needed.

## 6. Request mapping

  The existing provider-neutral contract is sufficient; no ModelPort, ModelRequest, or core change is proposed. crates/agent-core/src/model.rs:39 already preserves ordered roles and textual content.

  Mapping:

  ModelRole::System
    -> Message::System { content }

  ModelRole::User
    -> Message::User {
         content: OneOrMany::one(UserContent::Text(Text::new(content)))
       }

  ModelRole::Assistant
    -> Message::Assistant {
         id: None,
         content: OneOrMany::one(AssistantContent::Text(Text::new(content)))
       }

  Rules:

- Preserve exact message ordering.
- Preserve system messages in place; do not convert them into legacy preamble.
- Reject an empty ModelRequest as ModelPortError::Rejected, because Rig requires non-empty OneOrMany.
- Do not impose a speculative final-role restriction in M3. The portable Rig message type represents all three roles; provider-specific final-role restrictions belong to the later concrete provider
    integration.

- Construct CompletionRequest directly with:

  model                    = None
  preamble                 = None
  chat_history             = converted ordered messages
  documents                = []
  tools                    = []
  temperature              = None
  max_tokens               = None
  tool_choice              = None
  additional_params        = None
  output_schema            = None
  record_telemetry_content = false

  Direct construction avoids builder behavior that might reinterpret one message as a separately supplied prompt.

## 7. Response mapping

  Process Rig choices in their original order.

  AssistantContent::Text(text)
    -> ModelOutputPart::Text(text.text)

  Rig Text::additional_params, raw provider response, provider message ID, cache metadata, and reasoning metadata do not cross the adapter boundary.

  Usage mapping:

- A nonzero input_tokens becomes Some(input_tokens).
- A nonzero output_tokens becomes Some(output_tokens).
- Zero means unavailable and becomes None.
- If neither mapped value is available, return ModelResponse.token_usage = None.
- Never infer input/output counts from total_tokens.
- Cache, tool-use, and reasoning-token counters remain deliberately unmapped.

  Any unsupported response item makes conversion fail atomically. Mixed Text + unsupported content must not return partial text.

## 8. Tool-call treatment

  M3 will reject Rig tool-call responses as ModelPortError::Failed.

  Although the core contains ModelOutputPart::ToolCall, Rig’s tool call carries provider string IDs and optional provider call IDs, while the current domain uses a UUID crates/agent-core/src/ids.rs:59.
  Generating a new UUID would silently lose provider correlation.

  Therefore:

- No tools are registered with Rig.
- CompletionRequest.tools is empty.
- tool_choice is None.
- Rig ToolCall, Reasoning, and Image response items are unsupported.
- No Rig tool execution API is imported.

  A future provider-neutral external-call correlation field could enable safe mapping, but M3 does not add it prematurely.

## 9. Error/security mapping

  A private mapper consumes CompletionError and returns only the existing sanitized crates/agent-harness/src/ports.rs:25.

  Status-aware mapping:

  HTTP 4xx except 408/429 -> Rejected
  HTTP 408 or 429        -> Unavailable
  HTTP 5xx               -> Unavailable
  other preserved status -> Failed

  When no status is available:

  UrlError / RequestError          -> Rejected
  HttpError                        -> Unavailable
  JsonError / ResponseError        -> Failed
  ProviderError / ProviderResponse -> Failed
  future non-exhaustive variant    -> Failed

  Security rules:

- Never use Rig error Display or Debug in returned errors, events, or default tracing.
- Never expose response bodies, credentials, prompts, model output, or provider strings.
- Do not retain raw errors in adapter state.
- No adapter type containing the model or captured requests derives Debug.
- Fake request capture is test-support-only and is never audited or printed.

  Existing model audit events already contain only call IDs, budget metadata, and optional token usage. crates/agent-core/src/event.rs:90 requires no schema change.

## 10. Cancellation/retry semantics

  The adapter performs exactly one:

  model.completion(rig_request).await

  per ModelPort::invoke.

  It adds no:

- timeout
- cancellation token
- retry loop
- Tokio selection
- Rig AgentRunner hooks
- transport policy

  ExecutionHarness remains authoritative for preflight, model-call reservation, auditing, cancellation, and deadline selection. Its current tokio::select! wraps the port future at crates/agent-harness/src/
  execution.rs:338.

  Dropping the Rig future stops local awaiting but cannot prove that a future remote provider stopped processing. Remote cancellation acknowledgement and idempotency remain deferred.

  A future concrete model implementation might retry internally. Such behavior must be inspected and approved when that provider is introduced; the M3 fake performs no retries.

  Rig Agent, AgentRun, and AgentRunner are explicitly excluded. They could only be reconsidered later inside a bounded specialist node after defining non-competing authority for turns, tools, retries, policy,
  lifecycle, budgets, and audit.

## 11. Fake Rig model strategy

  Expose local deterministic support behind:

  [features]
  default = []
  test-support = []

  #[cfg(feature = "test-support")]
  pub mod testing;

  FakeRigModel implements the actual Rig 0.41.0 CompletionModel trait and supports:

- scripted text choices
- scripted Usage
- scripted CompletionError
- deterministic request capture
- invocation count
- pending completion future

  The required stream method returns an immediate test-only unsupported error; neither adapter nor CLI calls it.

  The fake uses no network, HTTP client, provider credentials, or separate test-support crate. The CLI enables agent-provider-rig/test-support.

## 12. Tests

  Adapter conversion:

- RigModelAdapter<FakeRigModel> implements ModelPort.
- Empty request is rejected without invoking the model.
- System/User/Assistant roles map correctly.
- Message ordering is exact.
- All unused CompletionRequest fields have their required safe values.
- Text choices and ordering map correctly.
- Zero, partial, and complete usage map without invented values.
- Raw response and message ID do not leak.

  Unsupported content and security:

- ToolCall, Reasoning, and Image each produce Failed.
- Mixed Text plus unsupported content fails atomically.
- Every Rig error class maps to the expected stable category.
- Status tests cover normal 4xx, 408, 429, 5xx, unusual 2xx error envelopes, and no-status transport errors.
- Prompt, response, provider-error, credential, and body sentinels appear in neither ModelPortError nor serialized audit events.

  Harness integration:

- One harness invocation consumes exactly one ModelCalls slot.
- Zero/exhausted ModelCalls budget never invokes the adapter.
- Pending completion is terminalized correctly by cancellation.
- Pending completion is terminalized correctly by elapsed deadline.
- Existing harness metadata ordering remains unchanged.

  Loop integration:

- One full M2 iteration invokes the model through LoopEffects → ExecutionHarness → dyn ModelPort.
- The Rig fake is invoked exactly once.
- The existing ReadOnly fake tool still runs through ToolPort.
- Verify passes, Reflect completes, and the run finishes Completed.

  Boundary verification:

- No Rig import in production sources of core, harness, or loop.
- Existing agent-loop dependency guard remains passing.
- cargo tree -i rig-core shows only agent-provider-rig and CLI paths.
- cargo tree -p agent-core, agent-harness, and agent-loop contains no Rig.
- The exact =0.41.0 pin and disabled default features are checked.

## 13. CLI demonstration

  Composition:

  start_run
  → LoopEngine
  → Observe
  → Retrieve
  → Plan
  → LoopEffects::invoke_model
  → ExecutionHarness::invoke_model
  → dyn ModelPort
  → RigModelAdapter
  → FakeRigModel
  → provider-neutral ModelResponse
  → Act
  → existing fake ReadOnly ToolPort
  → Verify(Passed)
  → Reflect(Complete)
  → Finished(Completed)

  Output remains limited to:

- RunId
- safe iteration/phase progression from metadata events
- final RunStatus
- ModelCalls usage
- ToolCalls usage
- Iterations usage
- audit_degraded
- compile-time static label such as rig-core / deterministic-fake

  No prompt, model response, tool input/output, Rig error, or captured request is printed.

## 14. Dependency/features

  Root workspace additions:

  [workspace.dependencies]
  agent-provider-rig = { path = "crates/agent-provider-rig" }
  rig-core = { version = "=0.41.0", default-features = false }

  agent-provider-rig production dependencies:

  [dependencies]
  agent-core.workspace = true
  agent-harness.workspace = true
  rig-core.workspace = true

  No production dependency on:

- rig
- rig-agent
- Tokio
- thiserror
- serde_json
- tracing
- provider clients
- MCP/RAG/vector crates

  Dev dependencies may include agent-loop, agent-harness/test-support, Tokio, and serde_json for deterministic integration tests.

  No Rig feature is enabled in M3. test-utils is unnecessary because the local fake needs pending behavior and precise request capture.

## 15. Risks and future extension points

- Rig API churn: exact pin plus a single adapter crate contains pre-1.0 breaking changes.
- Direct request construction: deliberately couples conversion code to 0.41.0 fields, but gives exact ordering and safe defaults.
- Transitive HTTP/runtime weight: rig-core still brings base HTTP/Tokio dependencies; dependency-tree evidence will make this explicit.
- Conversion loss: unsupported rich content fails explicitly instead of being discarded.
- Tool-call correlation: deferred until the domain can preserve provider call identity safely.
- Token concepts: Rig usage is telemetry; harness ModelCalls remains the governing budget.
- Cancellation: local future cancellation does not guarantee remote cancellation.
- Hidden retries: every future concrete provider implementation requires a retry-behavior audit.
- Streaming: requires a future provider-neutral streaming port; M3 does not call stream.
- Structured output: requires a provider-neutral schema/result contract before mapping Rig output_schema.
- Qwen gateway: future provider composition owns client/TLS/endpoint configuration outside core, harness, and loop.
- Rig AgentRun: potentially useful later inside an explicitly bounded specialist node, never as the outer runtime authority.

  Status: proposal only. No files were changed. Implementation awaits architectural approval.

# M3 web OpenAI review approvals

The M3 architecture proposal is approved with the following required
adjustments.

Implement M3 after applying these decisions.

1. Keep the selected architecture:

Use exactly:

rig-core = { version = "=0.41.0", default-features = false }

Do not depend on:

- rig facade
- rig-agent

Do not use:

- Agent
- AgentRun
- AgentRunner
- Rig tool execution
- Rig memory
- Rig RAG
- Rig MCP
- Rig retry/hook orchestration

The outer runtime remains:

LoopEngine
→ LoopEffects
→ ExecutionHarness
→ ModelPort
→ RigModelAdapter
→ rig_core::CompletionModel

1. Keep RigModelAdapter generic.

Use the approved shape conceptually:

RigModelAdapter<M>

where M implements the current rig_core::CompletionModel plus the bounds
required for ModelPort.

Do not attempt to store CompletionModel as a trait object because the current
Rig CompletionModel API is not dyn-compatible.

Type erasure remains at:

Arc<dyn ModelPort>

outside the adapter.

No Rig generic/type may leak into agent-core, agent-harness, or agent-loop.

1. Correct the CompletionError mapping.

Use stable semantics:

Provider response HTTP 4xx except 408/429
→ ModelPortError::Rejected

Provider response HTTP 408 or 429
→ ModelPortError::Unavailable

Provider response HTTP 5xx
→ ModelPortError::Unavailable

HttpError with no usable HTTP status
→ ModelPortError::Unavailable

UrlError
RequestError
JsonError
ResponseError
ProviderError without usable status
ProviderResponse without usable status
future/unknown non-exhaustive CompletionError
→ ModelPortError::Failed

Do not classify local URL parsing or request-construction failures as
Rejected.

Never expose:

- CompletionError Display
- CompletionError Debug
- provider body
- provider string
- URL
- credentials
- request content

through ModelPortError or AgentEvent.

1. Keep request conversion explicit and loss-aware.

Convert ordered provider-neutral ModelMessage values one-by-one.

Map:

System
→ Rig Message::System

User
→ Rig Message::User text content

Assistant
→ Rig Message::Assistant text content

Preserve exact message order.

Reject an empty ModelRequest before invoking the Rig model.

Do not normalize or reorder messages.

Do not move system content into Rig's legacy preamble.

Construct CompletionRequest directly and explicitly set every current
0.41.0 field.

For M3 the intended values are:

model = None
preamble = None
chat_history = converted non-empty messages
documents = []
tools = []
temperature = None
max_tokens = None
tool_choice = None
additional_params = None
output_schema = None
record_telemetry_content = false

Treat record_telemetry_content=false as a security invariant.

Add a test proving it remains false.

Document that direct struct construction is intentionally a Rig upgrade
tripwire: if a later Rig release changes CompletionRequest fields, compilation
should force explicit adapter review rather than silently accepting new
behavior.

1. Keep response conversion text-focused and atomic.

Process Rig AssistantContent in source order.

Map only supported textual content to:

ModelOutputPart::Text

Do not map:

- reasoning
- images
- provider-native content
- unknown future response content
- Rig tool calls

If ANY unsupported response item is present, fail the entire conversion with
sanitized ModelPortError::Failed.

Do not return partial text from a mixed supported/unsupported response.

1. Tool-call mapping remains deferred.

Even though agent-core currently has ModelOutputPart::ToolCall, do not invent a
UUID or discard Rig/provider call correlation merely to fit it.

No tools are supplied to Rig in M3.

tools = []
tool_choice = None

Actual tool execution remains exclusively:

LoopProgram
→ LoopEffects
→ ExecutionHarness
→ ToolPort

A future milestone will explicitly design provider-neutral model-proposed
action/tool-call correlation.

1. Usage mapping must follow Rig's documented zero-sentinel semantics.

Only map:

usage.input_tokens
usage.output_tokens

For each field:

- nonzero → Some(value)
- zero → None

If both are None:

- ModelResponse.token_usage = None

Do not infer usage from:

- total_tokens
- cached_input_tokens
- cache_creation_input_tokens
- tool_use_prompt_tokens
- reasoning_tokens

Do not add these provider/Rig telemetry concepts to agent-core in M3.

1. Keep raw Rig response data private.

Do not expose or retain beyond conversion:

- CompletionResponse.raw_response
- CompletionResponse.message_id
- provider response bodies
- provider-specific metadata

Do not add an accessor returning Rig response types.

1. Sensitive Debug/logging.

RigModelAdapter and fake/request-capture types containing model/request data
must not derive Debug unless explicitly redacted.

Do not emit request or response content through tracing.

The fake CLI must not print captured Rig CompletionRequest.

1. Cancellation and deadline authority remain in ExecutionHarness.

RigModelAdapter performs one:

model.completion(request).await

per ModelPort::invoke.

Do not add:

- timeout
- cancellation token
- tokio::select!
- retries
- retry hooks

inside the adapter.

Document that dropping the completion future stops local awaiting but cannot
prove remote provider work was cancelled.

1. No hidden retry semantics in the fake.

FakeRigModel executes exactly once per completion call.

Future real provider implementations must undergo a separate retry/idempotency
review in the provider/gateway milestone.

1. Fake Rig model.

Implement actual rig_core::CompletionModel for FakeRigModel behind test-support
or tests.

Support:

- scripted text response
- scripted Rig Usage
- scripted CompletionError
- request capture
- invocation count
- pending completion for cancellation/deadline integration tests

The required streaming method may return a deterministic sanitized unsupported
test error.

Do not panic or use unreachable! for the streaming method.

Do not create network clients.

1. Dependency language and verification.

agent-provider-rig itself should have no direct production Tokio dependency,
but document that rig-core 0.41.0 has transitive Tokio/reqwest/runtime
dependencies even with default features disabled.

Do not claim the resulting dependency graph contains no HTTP/Tokio crates.

The architectural requirement is that this transitive surface remains isolated
behind agent-provider-rig and does not enter the direct dependency manifests of
agent-core, agent-harness, or agent-loop.

1. Add explicit architecture boundary checks.

Verify production Rust sources under:

crates/agent-core
crates/agent-harness
crates/agent-loop

contain no imports/references to:

- rig_core
- rig::
- rig_agent

Also verify:

cargo tree -p agent-core
cargo tree -p agent-harness
cargo tree -p agent-loop

contain no Rig crates.

cargo tree -i rig-core

should show only approved paths through agent-provider-rig / CLI composition.

1. Tests must include all proposed tests plus:

- converted CompletionRequest.record_telemetry_content is always false
- direct request mapping leaves tools empty and tool_choice unset
- unsupported mixed Text + ToolCall/Reasoning/Image fails atomically
- UrlError/RequestError do NOT become Rejected
- HTTP status mapping distinguishes:
  400-class rejection
  408
  429
  500-class unavailable
- no Rig error Display/Debug sentinel leaks through ModelPortError
- no captured request/prompt/response sentinel leaks into AgentEvent
- production core/harness/loop source contains no Rig reference

1. Keep current M2 CLI architecture.

The deterministic CLI demonstration must be:

start_run
→ LoopEngine
→ Plan
→ LoopEffects::invoke_model
→ ExecutionHarness
→ dyn ModelPort
→ RigModelAdapter<FakeRigModel>
→ rig_core::CompletionModel
→ provider-neutral ModelResponse
→ Act through existing ToolPort
→ Complete

No real network access.

Print only safe metadata:

- RunId
- iteration/phase progression
- final RunStatus
- ModelCalls
- ToolCalls
- Iterations
- audit_degraded
- static adapter label if useful

Never print model request/response contents.

1. No production changes should be made to agent-core, agent-harness or
agent-loop unless actual compilation reveals a genuine provider-neutral
contract deficiency.

If such a deficiency appears:

- stop broad implementation
- make the smallest possible provider-neutral change
- explain it clearly in the final implementation report

Do not modify core merely to match Rig convenience APIs.

1. After implementation run:

cargo fmt --all -- --check

cargo clippy \
  --workspace \
  --all-targets \
  --all-features \
  -- -D warnings

cargo test --workspace --all-features

cargo tree --workspace
cargo tree -p agent-core
cargo tree -p agent-harness
cargo tree -p agent-loop
cargo tree -p agent-provider-rig
cargo tree -i rig-core

git diff --check

Also run an architecture source scan proving no Rig reference exists in
production sources of agent-core, agent-harness or agent-loop.

Do not commit automatically.

At the end report:

1. final changed tree
2. exact Rig dependency/version/features
3. RigModelAdapter implementation shape
4. request conversion
5. response conversion
6. usage conversion
7. unsupported-content handling
8. error mapping
9. security/telemetry behavior
10. cancellation/retry behavior
11. fake Rig model
12. integration with ExecutionHarness
13. integration with LoopEngine
14. dependency trees
15. architecture boundary scans
16. tests and quality gates
17. deliberate deferrals
18. deviations and justification

# M4

Proposal Codex

# M4 Architecture Proposal

  Decision: extend agent-provider-rig with a narrow OpenAI-compatible configuration and construction module. Do not create a gateway crate and do not change agent-core, agent-harness, agent-loop, ModelPort, or
  RigModelAdapter.

## 1. Inspected Rig 0.41 client APIs

  Inspected the committed rig-core = 0.41.0 source and published API.

  Use the Chat Completions client deliberately:

  use rig_core::{
      client::CompletionClient,
      providers::openai::CompletionsClient,
  };

  let client = CompletionsClient::builder()
      .api_key(credential)
      .base_url(base_url)
      .build()?;

  let model = client.completion_model(model_identifier);

  Exact relevant types:

- rig_core::providers::openai::CompletionsClient
  - Alias for Client<OpenAICompletionsExt, reqwest::Client>.

- rig_core::providers::openai::CompletionsClientBuilder
- rig_core::client::ClientBuilder
  - .api_key(...)
  - .base_url(...)
  - .http_headers(http::HeaderMap)
  - .http_client(...)
  - .build()

- rig_core::client::CompletionClient
  - .completion_model(model_identifier)

- Resulting model:
  - rig_core::providers::openai::completion::CompletionModel
  - Alias for GenericCompletionModel<OpenAICompletionsExt, reqwest::Client>.

  api_key means OpenAI-style bearer authentication: Rig inserts Authorization: Bearer <credential>. Custom headers are technically supported through http_headers; when that map already contains Authorization,
  Rig does not overwrite it.

  M4 should not use openai::Client, because in 0.41.0 that is the Responses API client. CompletionsClient selects /chat/completions, which is the broader vLLM/SGLang/gateway-compatible route.

  Rig joins the configured API root with /chat/completions, so these work:

- <http://127.0.0.1:8000/v1>
- <https://gateway.internal.example/v1>
- <https://oai.endpoints.kepler.ai.cloud.ovh.net/v1>

  The public API and non-dyn CompletionModel shape are confirmed in Rig’s 0.41.0 CompletionsClient documentation (<https://docs.rs/rig-core/latest/rig_core/providers/openai/client/type.CompletionsClient.html>) and
  CompletionModel documentation (<https://docs.rs/rig-core/latest/rig_core/completion/request/trait.CompletionModel.html>).

  Important dependency finding: the current default-features = false configuration has HTTP transport but no TLS backend. M4 must enable exactly Rig’s rustls feature for HTTPS endpoints.

## 2. Proposed file/tree changes

  Cargo.toml
  Cargo.lock

  crates/agent-provider-rig/
  ├── Cargo.toml
  ├── src/
  │   ├── lib.rs
  │   └── openai_compatible.rs
  └── tests/
      ├── openai_compatible_http.rs
      └── support/
          ├── mod.rs
          └── openai_server.rs

  apps/agent-cli/
  ├── Cargo.toml
  └── src/main.rs

  README.md

  Responsibilities:

- Existing crates/agent-provider-rig/src/lib.rs:31 retains RigModelAdapter.
- openai_compatible.rs owns validated configuration, secret handling, Rig client/model construction, and sanitized construction errors.
- Integration tests own the in-process HTTP server.
- CLI owns mode selection and environment loading.
- README documents the explicit live mode and security rules.

  No production changes are proposed under agent-core, agent-harness, or agent-loop.

## 3. Configuration model

  Proposed public provider-layer API:

  pub struct OpenAiCompatibleConfig {
      base_url: Url,
      model_identifier: String,
      credential: BearerCredential,
      provider_label: Option<ProviderLabel>,
  }

  impl OpenAiCompatibleConfig {
      pub fn new(
          base_url: impl AsRef<str>,
          model_identifier: impl Into<String>,
          credential: BearerCredential,
      ) -> Result<Self, OpenAiCompatibleConfigError>;

      pub fn with_provider_label(
          self,
          label: ProviderLabel,
      ) -> Self;
  }

  Configuration ownership:

- agent-cli
  - Reads environment variables.
  - Selects fake or live mode.
  - Constructs BearerCredential.
  - Passes raw configuration into the validated provider constructor.

- agent-provider-rig
  - Validates endpoint, model identifier, credential, and label.
  - Builds the Rig client/model.
  - Does not read environment variables.

- Core, harness, and loop
  - Know nothing about endpoints, credentials, providers, or model identifiers.

  Recommended live environment names:

  ELA_OPENAI_COMPAT_BASE_URL
  ELA_OPENAI_COMPAT_MODEL
  ELA_OPENAI_COMPAT_API_KEY
  ELA_OPENAI_COMPAT_LABEL       # optional

  No OVH-, Qwen-, vLLM-, or SGLang-specific environment variables belong in production code.

  The model identifier remains an opaque string. It may represent a deployed model, gateway alias, or virtual-model query.

## 4. Secret model

  Use a small local wrapper:

  pub struct BearerCredential(String);

  Properties:

- No Serialize or Deserialize.
- No Display.
- No AsRef<str> or public secret accessor.
- No derived Debug.
- Manual Debug produces BearerCredential(<redacted>).
- Prefer no Clone; construction consumes the credential.
- Empty or whitespace-only credentials are rejected.
- Credential contents never appear in configuration or construction errors.

  Add a compile-fail doctest proving serialization is unavailable, plus a runtime Debug-redaction test.

  A secret-management dependency is not justified in M4. Memory zeroization is deliberately deferred because Rig and HTTP header construction necessarily create additional copies; adding zeroization only to the
  initial wrapper would not provide an honest end-to-end guarantee.

## 5. Endpoint and TLS policy

  Validate before building the Rig client:

- URL must parse using url::Url.
- Scheme must be http or https.
- Host must be present.
- Username/password userinfo is rejected.
- Query and fragment are rejected.
- A URL already ending in /chat/completions is rejected; configuration must name the API root.
- Model identifier must be nonempty after trimming and contain no control characters.
- Credential is mandatory in M4.
- Optional label should be bounded and restricted to safe printable metadata.

  HTTP policy:

- http is permitted only for:
  - localhost
  - IPv4 loopback
  - IPv6 loopback

- Non-loopback HTTP is rejected by default.
- Non-loopback endpoints require HTTPS.
- 0.0.0.0 is not treated as loopback.

  Trailing slash handling:

- Accept both /v1 and /v1/.
- Do not rewrite arbitrary paths.
- Rig already joins both forms correctly with /chat/completions.
- Document that the supplied URL is an API-root URL.

  This allows local development without weakening the enterprise default.

## 6. Provider/model construction

  Expose one construction function:

  pub fn build_openai_compatible_model_port(
      config: OpenAiCompatibleConfig,
  ) -> Result<Arc<dyn ModelPort>, OpenAiCompatibleBuildError>;

  Internally:

  validated config
  → CompletionsClient::builder()
  → api_key
  → base_url
  → build
  → completion_model(configured identifier)
  → RigModelAdapter::new(model)
  → Arc<dyn ModelPort>

  This does not introduce another ModelPort implementation. It constructs and erases the existing generic RigModelAdapter.

  Client-builder failures become a stable sanitized error such as:

  OpenAiCompatibleBuildError::ClientConstructionFailed

  Do not retain the raw Rig/HTTP error as a public source if it could expose configuration.

  No readiness call is required in M4. Although Rig’s client implements VerifyClient using /models, many compatible endpoints do not implement that endpoint consistently. Normal startup should not depend on it.

  A future opt-in readiness command could call /models outside ExecutionHarness, consume no ModelCalls, and return only sanitized status.

## 7. Deterministic HTTP test architecture

  Use a private Tokio-based test server rather than adding WireMock, Axum, or another HTTP framework.

  TestOpenAiServer should:

- Bind 127.0.0.1:0.
- Accept a bounded scripted number of requests.
- Parse the request line, headers, Content-Length, and JSON body.
- Capture request data without deriving Debug.
- Return a scripted HTTP status and minimal OpenAI-compatible JSON.
- Set Connection: close.
- Shut down deterministically after the expected request count.

  Existing Tokio dev dependency gains only:

  features = ["test-util", "net", "io-util", "sync"]

  The server verifies:

- POST /v1/chat/completions
- Authorization: Bearer <sentinel>
- Configured model identifier
- Ordered System/User/Assistant messages
- No tools or tool choice
- Text response conversion
- Usage conversion
- HTTP error classification
- Malformed response handling

  No external network access is required.

## 8. Opt-in live architecture

  CLI modes:

  agent-cli
      default: deterministic FakeRigModel demo

  agent-cli --live-openai-compatible
      explicit real endpoint mode

  The default remains network-free and requires no environment variables.

  Live mode:

  1. Reads the four generic environment variables.
  2. Validates configuration before starting a run.
  3. Constructs Arc<dyn ModelPort> through the provider factory.
  4. Executes the existing deterministic loop and ReadOnly fake tool.
  5. Never prints the response body.

  Runtime path:

  start_run
  → LoopEngine
  → Plan
  → LoopEffects::invoke_model
  → ExecutionHarness
  → RigModelAdapter
  → CompletionsClient model
  → real endpoint
  → provider-neutral ModelResponse
  → deterministic Act
  → Verify
  → Reflect(Complete)
  → Finished(Completed)

  CI does not invoke this mode. No ignored live test is necessary if the CLI mode supplies the required operator validation path.

## 9. Error behavior

  Configuration errors occur before run startup:

- Malformed URL → sanitized configuration error
- Insecure remote HTTP → sanitized configuration error
- Empty model identifier → sanitized configuration error
- Empty credential → sanitized configuration error
- Rig client construction failure → sanitized build error

  Invocation behavior continues using M3 mappings:

   Condition                                        ModelPortError
  ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━  ━━━━━━━━━━━━━━━━
   400, 401, 403, 404, 422                          Rejected
  ───────────────────────────────────────────────  ────────────────
   408, 429                                         Unavailable
  ───────────────────────────────────────────────  ────────────────
   5xx                                              Unavailable
  ───────────────────────────────────────────────  ────────────────
   Connection/DNS/statusless HTTP failure           Unavailable
  ───────────────────────────────────────────────  ────────────────
   Malformed JSON                                   Failed
  ───────────────────────────────────────────────  ────────────────
   Valid HTTP status with invalid response shape    Failed
  ───────────────────────────────────────────────  ────────────────
   Unsupported tool/reasoning/image output          Failed

  The harness still reserves exactly one model call before invocation. Its deadline tokio::select! remains authoritative and drops the provider future on timeout or cancellation.

  No retries, fallback, backoff, or alternate model selection are introduced.

## 10. Security and logging

  Security invariants:

- Keep record_telemetry_content = false in the existing adapter.
- Do not log request or response bodies.
- Never log credentials or Authorization headers.
- Never place endpoint, credential, provider, or model configuration into RunContext, LoopState, or AgentEvent.
- Do not derive payload-bearing Debug on test captures or provider configuration.
- CLI prints only safe run metadata and an optional validated provider label.
- Full URLs should not be logged; if needed, expose only scheme plus host/port.
- Model identifiers may be treated as operational metadata, but the default CLI should not print them.

  Rig 0.41.0 contains full request/response body logging at the rig::completions TRACE target. M4 must explicitly keep the CLI subscriber capped above TRACE for that target and document that production
  compositions must not enable it. record_telemetry_content=false does not suppress those explicit TRACE statements.

## 11. Dependency changes

  Workspace:

  rig-core = {
      version = "=0.41.0",
      default-features = false,
      features = ["rustls"]
  }
  url = "2.5"

  agent-provider-rig production dependencies:

- agent-core — provider-neutral request/response types.
- agent-harness — ModelPort and sanitized port errors.
- rig-core — OpenAI-compatible client and completion model.
- url — explicit endpoint validation and loopback classification.
- thiserror — stable sanitized configuration/build errors.

  Dev-only:

- Existing Tokio, extended with net, io-util, sync, and test-util.

  No direct production dependencies on:

- reqwest
- http
- Axum/Hyper/WireMock
- config frameworks
- secret managers
- retry libraries
- OVH SDKs

  Rig will still transitively provide reqwest, Tokio, and Rustls.

## 12. Local Qwen path

  Example configuration:

  base URL: <http://127.0.0.1:8000/v1>
  model:    value advertised by the local vLLM/SGLang deployment
  token:    deployment-configured API key

  No Qwen model name is compiled into Rust.

  Changing from one Qwen version to another requires configuration only. The same applies to quantized variants, deployment aliases, or a new local serving engine.

  For M4, local deployments should be configured with an API key because Rig’s stock CompletionsClient requires bearer credential construction.

## 13. Optional OVH path

  OVH’s current OpenAI-compatible examples use:

- Base URL <https://oai.endpoints.kepler.ai.cloud.ovh.net/v1>
- Authorization: Bearer <OVH_AI_ENDPOINTS_ACCESS_TOKEN>
- Model supplied through the normal model field. OVH OpenAI-compatible example (<https://help.ovhcloud.com/csm/asia-public-cloud-ai-endpoints-function-calling?id=kb_article_view&sysparm_article=KB0071913>)

  Therefore, no OVH-specific code or custom header extension is needed.

  OVH virtual-model expressions can also remain opaque model identifiers—their documentation explicitly places these expressions in the normal OpenAI model field. OVH virtual-model documentation
  (<https://help.ovhcloud.com/csm/en-public-cloud-ai-endpoints-virtual-models%3Fid%3Dkb_article_view%26sysparm_article%3DKB0072094>)

  If a future gateway requires non-bearer authentication, introduce a reviewed typed authentication variant then. Do not expose arbitrary header maps prematurely.

## 14. Enterprise-gateway evolution

  Only composition configuration changes:

  M4 initial:
  agent → localhost vLLM/SGLang → Qwen

  Later:
  agent → internal OpenAI-compatible gateway
        → Qwen / Alibaba / another provider

  Unchanged:

- agent-core
- agent-harness
- agent-loop
- ModelPort
- RigModelAdapter
- deterministic loop transition authority
- harness budgets, cancellation, audit, and lifecycle

  A model identifier may be a concrete deployment, alias, or virtual model. The runtime does not interpret it.

  Routing, fallback, load balancing, retry policy, and provider selection remain gateway/future-milestone concerns.

## 15. Tests

  Required deterministic coverage:

- Valid localhost HTTP configuration.
- Malformed URL rejected.
- Empty model identifier rejected.
- Empty credential rejected.
- Non-loopback HTTP rejected.
- URL userinfo/query/fragment rejected.
- Secret Debug redacted.
- Secret cannot implement serialization, enforced by compile-fail test.
- Correct /v1/chat/completions path.
- Correct bearer header.
- Token does not leak through errors or events.
- Configured model identifier reaches request unchanged.
- Ordered message roles/content preserved.
- Tools absent and tool choice unset.
- Text completion converts successfully.
- Usage converts correctly.
- record_telemetry_content remains false.
- 401/403 → Rejected.
- 408/429 → Unavailable.
- 5xx → Unavailable.
- Malformed JSON/response → Failed.
- Model budget increments exactly once.
- Zero model budget prevents HTTP request.
- Harness cancellation/deadline terminalize a pending HTTP request.
- Full deterministic M2 loop completes against the local HTTP server.
- Existing ReadOnly tool remains harness-mediated.
- No prompt, response, credential, URL, or provider-error sentinel appears in AgentEvent or loggable public errors.
- Core/harness/loop production sources remain Rig/provider/network-free.
- Cargo trees confirm Rig and transport dependencies remain confined to the adapter/CLI path.
- Default CLI performs no network request.

## 16. Risks and tradeoffs

- Rig API churn: direct use of exact 0.41.0 builder types creates a useful compile-time upgrade tripwire.
- TLS surface: enabling rustls is necessary for enterprise/OVH HTTPS and expands the transitive dependency tree.
- Mandatory bearer auth: safe for enterprise and OVH, but requires local servers to accept/configure an API key.
- Internal certificates/proxies: custom CA bundles and proxy policy are not exposed in M4.
- Compatibility variance: “OpenAI-compatible” servers may differ in response fields or reasoning/tool-call behavior; unsupported content continues to fail atomically.
- TRACE leakage: Rig’s own TRACE body logging requires an explicit production logging restriction.
- Deadline semantics: dropping the future stops local awaiting but cannot prove remote provider cancellation.
- Live nondeterminism: real model output is intentionally excluded from CI assertions.
- Virtual model identifiers: a gateway alias can resolve to different models over time; that is allowed configuration behavior, not runtime routing.
- Readiness: omitting startup verification avoids dependence on inconsistent /models endpoints but means the first completion discovers connectivity failure.
- Custom authentication: arbitrary/custom header authentication is deferred until a concrete gateway requires it.
- Tool calls, streaming, structured output, retries, routing, and fallback remain deliberately deferred.

No files were edited. Awaiting architectural approval.

# M4 web openai review approvals

The M4 architecture proposal is approved with the following required
refinements.

Implement M4 now.

1. Preserve the approved architecture.

Do not modify:

- agent-core
- agent-harness
- agent-loop
- ModelPort
- RigModelAdapter

unless actual compilation reveals a genuine provider-neutral deficiency.

M4 belongs in:

- agent-provider-rig
- agent-cli composition
- deterministic integration-test infrastructure
- documentation

No gateway crate.

1. Use Rig 0.41 Chat Completions exactly as proposed.

Use:

rig-core = {
    version = "=0.41.0",
    default-features = false,
    features = ["rustls"]
}

Use:

- rig_core::providers::openai::CompletionsClient
- rig_core::client::CompletionClient
- completion_model(...)

Do not use:

- OpenAI Responses API client
- Rig Agent
- AgentRun
- AgentRunner
- Rig tool execution
- retries
- streaming

1. Bearer authentication remains mandatory in M4.

Keep BearerCredential as the provider config credential.

The wrapper:

- must not implement Serialize/Deserialize
- must not implement Display
- must use redacted Debug
- preferably does not Clone
- exposes no public raw-secret accessor
- rejects empty/whitespace-only values

The provider module itself may consume/access its private secret value to pass
it into Rig.

Do not introduce Authentication enums, custom headers, secret frameworks or
no-auth modes yet.

1. Configuration ownership remains:

agent-cli:

- reads environment/configuration
- selects fake/live mode

agent-provider-rig:

- validates provider configuration
- builds Rig client/model
- owns secret wrapper

core/harness/loop:

- remain unaware of endpoint/model/credential/provider configuration.

1. URL validation.

Use url::Url.

Require:

- http or https
- host present
- no username/password
- no query
- no fragment

Reject API roots ending in:

- /chat/completions
- /chat/completions/

Only normalize an optional trailing slash on the API-root path.

Do not otherwise rewrite:

- host
- scheme
- port
- arbitrary path segments.

Test both:

- /v1
- /v1/

and prove both produce:
POST /v1/chat/completions

1. HTTP transport policy.

HTTP is allowed only for loopback.

Determine loopback using URL host semantics:

- domain exactly localhost (case-insensitive)
- IPv4 addr.is_loopback()
- IPv6 addr.is_loopback()

Do not treat:

- 0.0.0.0
- RFC1918/private IPv4
- arbitrary internal DNS names

as loopback.

Non-loopback endpoints require HTTPS.

Do not add an insecure-remote override in M4.

1. Model identifier.

Treat as opaque validated String.

Validation:

- non-empty after trimming
- no control characters

Do not hard-code Qwen/OVH model constants.

Do not parse gateway aliases or virtual-model syntax.

Pass the configured value unchanged into completion_model().

1. Provider label.

Keep ProviderLabel optional and safe.

Bound its length and restrict it to safe printable metadata.

Do not treat it as security-sensitive configuration.

It may be printed by live CLI.

Do not print model identifier by default.

1. Keep:

build_openai_compatible_model_port(
    OpenAiCompatibleConfig
) -> Result<Arc<dyn ModelPort>, OpenAiCompatibleBuildError>

Internally:

validated config
→ CompletionsClient builder
→ api_key
→ base_url
→ build
→ completion_model
→ RigModelAdapter
→ Arc<dyn ModelPort>

Do not create another ModelPort wrapper.

1. Error sanitation.

Configuration/build errors must not contain:

- credential
- Authorization header
- full URL
- request/response body
- provider raw errors

Use stable typed variants.

Do not publicly expose underlying Rig/reqwest errors as Error::source if doing
so may reveal configuration or transport bodies.

1. Security logging.

Keep the existing Rig request invariant:

record_telemetry_content = false

Add/retain a test for it.

Do not assert in documentation that Rig 0.41 necessarily logs full
request/response bodies at a specific TRACE target unless that is proven from
the exact pinned source.

Instead document:

Production compositions must not enable TRACE-level Rig/provider logging
without first reviewing the exact pinned Rig version for content leakage.

The CLI should use conservative metadata-only logging.

1. Test HTTP server.

Implement the proposed private Tokio test server.

It is not a general HTTP server.

Bound it explicitly:

- fixed small expected request count
- maximum header size
- maximum Content-Length/body size
- bounded read/accept timeout
- Content-Length requests only
- Connection: close
- no chunked transfer decoding
- no keep-alive support
- no HTTP/2

Unexpected protocol behavior should fail the deterministic test.

Do not add Axum, Hyper, WireMock or another web framework merely for M4 tests.

1. Deterministic HTTP tests must verify:

- /v1 and /v1/ both lead to /v1/chat/completions
- exact configured model field
- Authorization: Bearer ...
- ordered System/User/Assistant messages
- no tools/tool-choice
- successful textual completion
- token usage conversion
- 401/403 -> Rejected
- 408/429 -> Unavailable
- 5xx -> Unavailable
- malformed JSON -> Failed
- malformed success response -> Failed
- secret sentinel does not appear in public errors/events
- prompt/response sentinel does not appear in AgentEvent

1. Preserve harness authority.

The provider factory/model must add no:

- timeout
- retry
- cancellation token
- fallback

ExecutionHarness remains authoritative for:

- ModelCalls
- deadline
- cancellation
- audit
- lifecycle.

Cancellation of the local future does not prove remote cancellation.

1. Full deterministic integration test.

Exercise:

start_run
→ LoopEngine
→ Plan
→ LoopEffects
→ ExecutionHarness
→ ModelPort
→ RigModelAdapter
→ CompletionsClient
→ local deterministic HTTP server
→ textual response
→ existing deterministic ReadOnly Act
→ Verify
→ Reflect(Complete)
→ Finished(Completed)

Assert usage:

- ModelCalls = 1
- ToolCalls = 1
- Iterations = 1

1. Live CLI mode.

Default mode remains network-free.

Add explicit:

--live-openai-compatible

without adding Clap unless there is a concrete reason.

Environment variables:

ELA_OPENAI_COMPAT_BASE_URL
ELA_OPENAI_COMPAT_MODEL
ELA_OPENAI_COMPAT_API_KEY
ELA_OPENAI_COMPAT_LABEL optional

Only live mode reads/requires these values.

Validate configuration before start_run.

1. Keep live-loop control deterministic.

Plan may invoke the real model.

Verify must only inspect provider-neutral typed facts needed for success, for
example:

- at least one textual ModelOutputPart exists
- Act produced the expected typed ReadOnly result

Reflect must produce deterministic Complete for the M4 smoke path.

Do not use another model invocation for Verify/Reflect.

Do not let free-form model output decide the outer loop transition.

1. CLI live output may contain only safe metadata:

- RunId
- provider label
- iteration/phase progression
- final RunStatus
- ModelCalls
- ToolCalls
- Iterations
- audit_degraded

Do not print:

- prompt
- response
- token
- full URL
- model identifier by default
- captured HTTP request

1. Readiness remains deferred.

Do not call /models during normal startup.

Do not spend ModelCalls on connectivity checks.

No new health abstraction.

1. HTTPS support.

Enable exactly Rig's rustls feature.

Do not add:

- native-tls
- proxy configuration
- custom CA
- mTLS

in M4.

Document those as enterprise deployment extensions.

1. Test build/security surface.

Keep:

- url as production dependency if needed for validation
- thiserror for typed provider construction errors

Do not add:

- reqwest directly
- http directly
- config framework
- secret manager
- retry library
- OVH SDK

Tokio net/io/sync features may be dev-only for deterministic HTTP tests.

1. Boundary checks.

Verify no changes/imports of provider/network/Rig concerns in:

- agent-core
- agent-harness
- agent-loop

Run:
cargo tree -p agent-core
cargo tree -p agent-harness
cargo tree -p agent-loop
cargo tree -p agent-provider-rig
cargo tree -i rig-core

Also run the existing architecture source scan.

1. Required quality gates:

cargo fmt --all -- --check

cargo clippy \
  --workspace \
  --all-targets \
  --all-features \
  -- -D warnings

cargo test --workspace --all-features

git diff --check

Run the deterministic/default CLI mode.

Do NOT execute the live external mode automatically.

Do not commit.

1. Final implementation report must include:

1. changed tree
1. exact Rig feature/dependency change
1. OpenAiCompatibleConfig API
1. BearerCredential behavior
1. endpoint normalization/security policy
1. provider factory
1. deterministic HTTP server design
1. HTTP integration test results
1. full loop HTTP integration result
1. live CLI mode/configuration
1. security/logging behavior
1. dependency-tree verification
1. core/harness/loop boundary verification
1. quality gates
1. deliberate deferrals
1. any deviations and justification
