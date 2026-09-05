---
title: "M7 Durable Event Persistence and Recovery"
tags: ["m7", "persistence", "sqlite", "checkpoint", "recovery", "event-journal"]
created: 2026-09-05
updated: 2026-09-05
sources: ["crates/agent-harness/src/persistence.rs", "crates/agent-harness/src/recovery.rs", "crates/agent-persistence-sqlite/src/lib.rs", "M7 approved architecture and implementation"]
links: ["enterprise-local-agent-milestone-index.md", "m6-1-production-linux-localwrite-containment.md"]
category: architecture
confidence: high
schemaVersion: 1
---

# M7 Durable Event Persistence and Recovery

## Status

Implemented and verified in the working tree. No M7 commit or tag has been
created. M7 provides a durable metadata journal, quiescent checkpoints, recovery
classification, and explicitly versioned restart support without replaying
external effects.

## Scope and Core Invariant

Replay reconstructs provider-neutral runtime metadata only. It never invokes
`ModelPort`, `ApprovalPort`, `ToolPort`, `ContainedToolPort`, or LocalWrite.
M7 does not claim generic recovery of arbitrary `LoopProgram::WorkingState`,
exactly-once execution, durable human approval queues, provider retries, or
distributed persistence.

The dependency direction is:

```text
agent-persistence-sqlite
    -> agent-harness
    -> agent-core
```

SQLite and filesystem concerns remain outside core and harness. The
`ExecutionHarness` remains the authoritative owner of lifecycle, budgets,
policy, approval, required audit, containment, cancellation, and deadlines.

## Persistence Boundary

Harness defines the provider-neutral `RunPersistencePort` with `create_run`,
`append_transition`, and `load_run`. `ExecutionHarness` may own this port through
composition. Persistence DTOs contain only safe metadata: run/session identity,
budget and usage, status, audit degradation, wall-clock start metadata, event
sequence, loop position, continuation classification, and effect correlation
identifiers.

Prompts, action arguments, model responses, approval previews and objects,
`ValidatedAction`, tool results, credentials, runtime ports, futures,
cancellation tokens, monotonic clocks, and runtime handles are never persisted.

## Journal and Checkpoint

The journal stores ordered, schema-versioned `AgentEvent` records with exact
`EventSequence` continuity. SHA-256 checksums protect serialized records and a
chained digest binds event order. Recovery rejects sequence gaps, duplicates,
wrong run/session identity, corruption, inconsistent checkpoints, and
unsupported store, checkpoint, or event versions.

Checkpoints are written only when no external effect is unresolved. They store
the minimum provider-neutral `DurableRunState`. Recovery deterministically
replays the checkpoint prefix and compares the entire resulting state with the
stored checkpoint before replaying the tail through the same reducer used by
live execution.

## Durable-Before-Effect

Every model, approval, tool, and contained LocalWrite dispatch follows:

```text
durable InvocationStarted
-> required audit
-> dispatch
```

If the start transition cannot be committed, execution fails closed and the
external port is not invoked. Once a durable start exists, recovery treats a
missing trustworthy terminal event as `ManualReconciliationRequired`; it never
automatically retries the uncertain effect. The persistence journal remains
separate from the M6 audit sink, and no atomic transaction across both systems
is claimed.

## Recovery Classification

Recovery validates versions, identity, checksums, chain digest, exact sequence,
checkpoint consistency, budgets, loop transitions, and pending effects. It then
returns exactly one disposition:

- `Completed`
- `TerminalFailure`
- `Resumable`
- `ManualReconciliationRequired`

Only a program implementing `RestartableLoopProgram` with a matching recovery
version may receive `Resumable`, and only at an initial or explicit restartable
boundary. Restoration creates a fresh process-local `RunContext`. Elapsed budget
is conservatively reconstructed from trusted wall-clock metadata; rollback or
ambiguity fails closed.

## SQLite Adapter

`agent-persistence-sqlite` uses `rusqlite 0.40.2` with bundled SQLite. The first
implementation is single-process, uses rollback journal mode,
`synchronous=FULL`, foreign keys, an immediate transaction, and append-only
event update/delete triggers. One per-adapter semaphore bounds blocking SQLite
work admitted through `tokio::task::spawn_blocking`.

Expected-sequence comparison, next event append, optional checkpoint update,
and terminal-state update occur in one transaction. The implementation makes no
multi-process, distributed, authenticated-journal, or filesystem-beyond-SQLite
durability claim.

## Schema Decision

`AgentEvent` advances from schema version 5 to 6 because deterministic recovery
requires metadata-only `RunStarted` wall-clock data and durable
`AuditDegraded`. Unsupported versions are rejected; there are no automatic
migrations or best-effort interpretations.

## Verification

Formatting, strict workspace Clippy, dependency checks, graph change detection,
and `git diff --check` passed. The workspace suite passed 189 tests and four
doctests. Coverage includes full-log and checkpoint-tail recovery, sequence and
identity failures, corruption and unsupported versions, budget and audit state,
terminal restoration, every loop phase, unresolved model/approval/tool effects,
zero-invocation persistence failure, pure replay, explicit restartability, and
all-or-nothing SQLite transaction fault injection.

The explicit M6.1 Linux certification also passed with bubblewrap 0.11.2, a
mode-0555 static PIE musl worker, and partially enforced optional Landlock. It
covered governed create/replace, escape and network controls, environment and FD
isolation, process-tree reaping, ToolCallId correlation, and recovery without
replaying LocalWrite.

## Forward Constraints

- Do not treat the persistence journal as the M6 audit sink.
- Do not deserialize process-local runtime internals.
- Do not resume transient or arbitrary working state.
- Do not retry any invocation with uncertain external-effect status.
- Preserve M6 policy, approval binding, required audit, budgets, containment,
  LoopEngine ownership, and provider/Rig boundaries.
