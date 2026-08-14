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
