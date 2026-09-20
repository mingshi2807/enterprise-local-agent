---
title: "M14 Deployment and Operations Hardening"
tags: ["m14", "deployment", "operations", "readiness", "backup", "restore"]
created: 2026-09-20
updated: 2026-09-20
sources: ["crates/agent-deployment/src/lib.rs", "apps/agent-operator/src/main.rs", "apps/agent-service-daemon/src/main.rs", "crates/agent-service/src/lib.rs", "crates/agent-service-http/src/lib.rs", "crates/agent-persistence-sqlite/src/lib.rs"]
links: ["enterprise-local-agent-milestone-index.md", "m13-local-enterprise-agent-mvp.md", "m15-enterprise-identity-authorization.md", "m12-agent-service-api.md", "m7-durable-event-persistence-and-recovery.md", "m6-1-production-linux-localwrite-containment.md"]
category: architecture
confidence: high
schemaVersion: 1
---

# M14 Deployment and Operations Hardening

## Status

Implemented, verified, and committed as
`edb3bd0d017e25a5b40b37681d1e08717096006a` (`feat: add M14 deployment and
operations hardening`). No M14 tag has been created.

## Boundary

M14 adds deployment composition and operations without adding agent capability:

```text
agent-service-daemon
  -> agent-service-http
  -> agent-service
  -> graph/loop
  -> ExecutionHarness

agent-deployment -> trusted composition and offline administration
agent-operator   -> operational APIs only
```

`ExecutionHarness` remains authoritative for effects, policy, approval, audit,
budgets, cancellation, containment, persistence ordering, and recovery.
`agent-deployment` does not expose runtime ports or execute workflows.

## Versioned Configuration

M14 introduced `DeploymentConfigV1`, bounded to 64 KiB with unknown-field
rejection. M15 advances the active configuration to schema 2 and
`DeploymentConfigV2`; it validates
listener security, storage/audit names, workflow profiles, model endpoint and
explicit auth mode, knowledge route, MCP process/fingerprints, LocalWrite
workspace/artifacts, secret references, and operational limits.

Secrets are supplied through absolute external file references. Resolution
requires a regular file owned by the service effective user with no group/world
permissions. Secret values use redacted debug output and are excluded from the
deterministic SHA-256 deployment fingerprint. The checked-in ReadOnly example
is now `docs/deployment-config-v2.example.toml`.

## Profiles and Readiness

ReadOnly and LocalWrite are independent profiles. The daemon performs bounded
model and knowledge diagnostics during startup and publishes cached readiness;
it does not continuously run inference or retrieval for health checks.
`/healthz` reports only process lifecycle.

LocalWrite additionally requires the configured seal key ID, workspace binding,
derived tool-contract digest, worker protocol, trusted artifact hashes and
modes, and the existing M6.1 functional capability probe. A failed LocalWrite
probe omits only that workflow and reports it unavailable. It never downgrades
LocalWrite to ReadOnly. Runtime readiness is a deployment check, not universal
containment certification.

## Observability

Operational HTTP views expose cached dependency/workflow readiness, build and
schema versions, fixed-bucket run/model/retrieval/tool/approval timings, bounded
counters, and metadata-only run/reconciliation summaries. Metrics do not use
RunId labels and contain no prompts, answers, evidence, paths, action content,
tool payloads, credentials, capsules, or audit bodies.

Run completion metrics use durable recovery classification after callbacks, so
intentional Waiting is not reported as completed. Approval waits observed
entirely in one process have a timing; a wait spanning restart has no complete
duration because M10 deliberately persists no approval timestamp.

## Draining and Recovery

`AgentService` owns tracked run tasks and exposes `Serving` and `Draining`.
Draining rejects new sessions, starts, resumes, decisions, and abort requests;
signals existing harness cancellation handles; waits within a configured bound;
and reports remaining active work. The daemon flushes audit output before
releasing its exclusive data-directory lock.

Durable Waiting is quiescent and is not cancelled by shutdown. Started effects
without trustworthy terminal records retain M7
`ManualReconciliationRequired`. M14 does not manufacture a terminal status or
retry an effect during shutdown or startup.

## Backup and Restore

`SqliteStoreAdmin` is an offline/quiesced adapter. It uses the SQLite backup API
under exclusive deployment ownership and never raw-copies a live database. The
audit file is copied only while quiesced. Database and audit files are mode
0600, fsynced, hashed, and named in `BackupManifestV1`.

The manifest separates informational build/git identity from critical store,
event, checkpoint, workflow/program, graph, capsule, tool-contract, key,
workspace, MCP, containment protocol, and artifact identities. Verification
runs SQLite integrity and foreign-key checks, validates every durable run and
approval record, and rejects unsupported or mismatched contracts. A different
build may restore when all critical contracts match.

Restore requires an empty target, verifies before activation, rejects symlinked
database/audit sources, and removes partial output on failure. Hashes detect
corruption and accidental tampering; no signed-backup or authenticity guarantee
is claimed. There is no automatic migration or downgrade.

## LocalWrite Deployment Integrity

The daemon checks absolute artifact paths, configured SHA-256 values, root
ownership, executable and immutable modes, absence of setuid/setgid bits,
worker protocol, workspace binding, seal key ID, tool-contract digest, and the
bounded M6.1 probe before registering LocalWrite.

The explicit certification rebuilt the worker from the current source as a
static PIE ELF. The certified artifacts were:

- Bubblewrap 0.11.2, SHA-256 `52461e70c92497b12ebe637eceb0a651b9cc66251a39f45afb755f84dd815581`;
- root-owned mode-0555 worker, SHA-256 `338dc225d68431e0df50160f48a12aa829e55721115af40562fb3b0f54cddae2`;
- Landlock status `PartiallyEnforced`.

The non-skipping `production_linux_security_certification` passed. Landlock
remains optional defense-in-depth.

## Operator CLI

`agent-operator` provides only config validation, cached readiness, version,
run list/show, reconciliation list/show, backup create/verify, and restore
verify/apply. Online reads use the configured Unix socket or authenticated
loopback listener. Offline mutations require exclusive deployment ownership.

The operator has no approval, resume, policy mutation, workflow execution, or
runtime-port API. It cannot bypass `ExecutionHarness`.

## Verification

The final worktree passed:

- `cargo fmt --all -- --check`;
- strict workspace Clippy with all targets and features;
- 299 workspace tests and four compile-fail doctests;
- SQLite transaction-fault, corruption, backup/restore, tamper, and cross-build tests;
- service draining, ownership, readiness cache, metadata leak, HTTP/SSE, M7-M13 regression tests;
- dependency and forbidden-authority scans plus `git diff --check`;
- Codebase Memory blast-radius and coverage checks;
- explicit M6.1 certification with one passed, zero failed, zero ignored.

Normal workspace tests continue to ignore the three explicit M13 real-service
smokes and the separately invoked M6.1 certification by design.

## Guarantees and Limits

M14 provides strict server-controlled configuration, profile-specific readiness,
metadata-only operational inspection, bounded graceful draining, consistent
offline backups, fail-closed compatibility validation, and deployment artifact
integrity checks without moving execution authority.

It does not provide HA, distributed workers, multi-process coordination,
online backup, model lifecycle management, automatic migration, SSO/RBAC,
signed backup authenticity, or exactly-once effects. Configured MCP servers not
used by the M13 workflows are reported as configured/degraded rather than
falsely reported usable.

## Forward Constraints

- Keep `agent-deployment` above service/harness authority boundaries.
- Never turn health/readiness or operator commands into execution paths.
- Keep telemetry fixed-cardinality and payload-free.
- Require exclusive ownership for offline backup and restore.
- Fail closed on incompatible durable or security contracts.
- Preserve independent ReadOnly and LocalWrite readiness with no downgrade.
- Keep runtime containment probes distinct from release certification.
