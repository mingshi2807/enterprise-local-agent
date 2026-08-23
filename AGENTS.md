# Enterprise Local Agent working contract

## Mission and current scope

Build a local-first enterprise LLM agent platform in stable Rust Edition 2024.
Implement only the requested vertical milestone. Do not add RAG, MCP, graph
orchestration, vector databases, web servers, distributed execution, or other
future layers unless the current milestone explicitly requires them.

Current architecture and milestone behavior are documented in `README.md`,
`docs/proposal/proposal.md`, and `omx_wiki/`. Those files are context, not
authorization to expand the active milestone.

## Architectural boundaries

- Domain and execution-policy APIs remain independent from Rig, Qwen, OpenAI,
  cloud vendors, MCP, graph frameworks, model providers, and vector databases.
- Provider and framework types do not leak into core domain APIs.
- Dependency direction is inward: domain <- harness <- loop <- adapters.
- The enterprise harness owns session/context, identity, capability policy,
  approval, budget, audit, containment policy, cancellation, timeout,
  streaming, and observability.
- The runtime owns bounded outer-loop transitions. An LLM may provide typed
  decisions inside a step but never owns the outer execution loop.
- Tools are typed as ReadOnly, LocalWrite, ExternalWrite, or Privileged.
  Non-read-only execution must follow the milestone's policy, approval, and
  containment contracts.

## Rust rules

Use Tokio for async, serde for serialization, thiserror for library/domain
errors, anyhow only at application composition boundaries, tracing for
structured observability, and uuid for runtime identifiers.

Prefer explicit types, enums for state machines, identifier newtypes,
immutable transitions where practical, dependency inversion, composition, and
small focused traits.

Production code must not use `unwrap()`, `expect()`, recoverable `panic!()`,
unjustified `unsafe`, global mutable state, hidden side effects, unbounded loops
or channels, stringly typed state machines, or framework types in domain APIs.

Potentially blocking or external operations need explicit timeout behavior,
cancellation where practical, structured concurrency, and no mutex guard held
across an await point.

## Security and testing

- Never commit credentials, keys, tokens, private certificates, or secrets.
- Do not include credential values or payload-bearing provider data in logs or
  errors.
- Core and harness tests are deterministic and require no real model, network,
  cloud service, or MCP server. Use fakes at architecture boundaries.
- During implementation, run the smallest targeted checks that prove changed
  behavior.
- Before declaring a milestone complete, run:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

Use `CARGO_HOME=/tmp/enterprise-local-agent-cargo` when the normal Cargo cache
is not writable.

## Efficient Codex execution

- Default to direct solo work. Do not delegate lookup, explanation, one-file
  changes, or routine verification.
- Delegate only independent, substantial lanes with distinct deliverables;
  use at most three children and no nested delegation.
- OMX workflows require explicit `$name` invocation. Do not infer persistent or
  parallel workflows from ordinary language.
- Use low effort for lookup, medium for routine implementation and review, and
  high only for consequential architecture, security, or adversarial critique.
- For small changes, verify once with targeted checks. Add independent review
  only for public API, security, cross-crate, or otherwise high-risk changes.
- Stop searching when the task is grounded. Stop testing when sufficient fresh
  evidence proves the requested result. Do not rerun passing checks without a
  changed artifact or new failure evidence.

## Change protocol

Before broad changes, inspect the current implementation, state the proposed
design and affected boundaries, then implement only the requested milestone.
Do not silently alter architecture to simplify implementation. Inspect current
official or installed APIs when external-library behavior is uncertain.

After changes, report modified files, important decisions, verification
evidence, deferred work, and deviations. Do not commit unless explicitly asked.

## Codebase discovery

For architecture-sensitive tasks, unfamiliar subsystems, cross-crate
changes, refactoring, or impact analysis, prefer codebase-memory-mcp
before broad grep/read exploration.

Recommended sequence:

1. get_architecture for unfamiliar system-level tasks.
2. search_graph to locate symbols and relationships.
3. trace_path for caller/callee analysis.
4. get_code_snippet or direct file reads only after structural discovery.
5. detect_changes after non-trivial implementation changes.
6. consult relevant ADRs before altering architectural boundaries.

Do not use graph lookup mechanically for trivial local edits where the
target file and symbol are already known.
