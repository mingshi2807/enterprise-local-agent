09 / 08 / 2026

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
