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

# review version by OpenAI web

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
