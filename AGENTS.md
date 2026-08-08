# Project Mission

Build a local-first, enterprise-grade LLM agent platform in Rust.

The target architecture is:

Client Layer
    ↓
Enterprise Agent Harness
    ↓
Deterministic Loop Engine
    ↓
Graph Engine
    ↓
Model / Tools / Knowledge

The platform will initially use locally/self-hosted frontier models such as
Qwen through an OpenAI-compatible enterprise AI gateway.

Rig is an infrastructure adapter for model and agent integration.
Rig is NOT the architecture of this application.

# Architectural Boundaries

The domain and execution-policy layers must remain independent from:

- Rig
- Qwen
- OpenAI
- OVH
- Alibaba Cloud
- MCP
- graph-flow
- any specific model provider
- any specific vector database

Provider/framework-specific types must not leak into core domain APIs.

Prefer dependency inversion:

domain
    <- harness
    <- loop
    <- adapters

Adapters depend inward.
Core layers never depend outward.

# Development Strategy

Implement the system as small vertical milestones.

Do not build the entire platform at once.

Current milestone requirements always take precedence over future architecture.

Do not prematurely add:

- RAG
- MCP
- multi-agent systems
- graph orchestration
- vector databases
- web servers
- Tauri
- distributed execution

unless explicitly requested by the current milestone.

# Rust

Use:

- stable Rust
- Rust Edition 2024
- Tokio for async
- serde for serialization
- thiserror for library/domain errors
- anyhow only at application composition boundaries
- tracing for structured observability
- uuid for runtime identifiers

Prefer:

- explicit types
- enums for state machines
- newtypes for identifiers
- immutable state transitions where practical
- dependency inversion
- composition over inheritance-like abstractions
- small focused traits

Avoid:

- unwrap() in production code
- expect() in production code
- panic!() for recoverable failures
- unsafe unless explicitly justified
- global mutable state
- hidden side effects
- unbounded loops
- unbounded channels
- stringly typed state machines
- framework types in domain APIs

# Async

All potentially blocking/external operations must:

- support cancellation where practical
- have explicit timeout behavior
- avoid holding mutex guards across await points

Prefer structured concurrency.

# Core Domain

The project should eventually expose provider-independent concepts including:

RunId
SessionId
UserId
RunContext
RunBudget
RunStatus
RunOutcome
AgentEvent
Capability
CapabilitySet
CapabilityPolicy
ApprovalRequest
ApprovalDecision
ToolCall
ToolResult
ModelRequest
ModelResponse

Do not add all of them unless required by the current milestone.

# Agent Harness

The enterprise harness owns:

Session
Context
Identity
Tool registry
Capability policy
Approval
Budget
Audit
Sandbox policy
Cancellation
Timeout
Streaming
Observability

These concerns must not be delegated entirely to Rig.

# Loop Engine

The target logical loop is:

Observe
→ Retrieve
→ Plan
→ Act
→ Verify
→ Reflect

The runtime controls transitions.

The LLM may provide decisions or structured outputs inside a step, but the
LLM must not own the outer execution loop.

Every loop must be bounded by explicit execution policy.

# Graph Engineering

Graph orchestration is a future layer.

The graph engine will own:

- typed state
- nodes
- conditional edges
- fan-out/fan-in
- checkpointing
- HITL
- durable execution
- resume/replay

Do not introduce graph-flow or another workflow framework before the
deterministic single-agent loop is proven.

# Model Integration

Model access eventually goes through:

Agent Runtime
    ↓
ModelPort
    ↓
RigModelAdapter
    ↓
EnterpriseAIGateway
    ↓
Model Provider

The domain layer must not import Rig types.

# Tools

Tools are typed capabilities.

Every tool must be classified as one of:

ReadOnly
LocalWrite
ExternalWrite
Privileged

Initially implement ReadOnly capabilities only.

All non-read-only tool execution will eventually pass through policy and,
when necessary, human approval.

# Security

Never commit:

- API keys
- access tokens
- credentials
- private certificates
- secrets

Secrets come from environment variables or a future secret provider.

Do not include credential values in logs or errors.

# Testing

Core and harness tests must be deterministic.

Core tests must never require:

- a real LLM
- network access
- OVH
- Alibaba Cloud
- an MCP server

Use fake implementations at architecture boundaries.

Before considering a milestone complete, run:

cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features

# Codex Working Rules

Before making broad changes:

1. Read AGENTS.md.
2. Inspect the existing repository.
3. Explain the proposed design.
4. Identify architectural boundary changes.
5. Implement only the requested milestone.
6. Run formatting, Clippy and tests.
7. Summarize changed files and important decisions.

Do not silently change architecture to make implementation easier.

When uncertain about an external library API, inspect the currently installed
or current published API rather than assuming an older API.
