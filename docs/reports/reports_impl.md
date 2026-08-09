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
