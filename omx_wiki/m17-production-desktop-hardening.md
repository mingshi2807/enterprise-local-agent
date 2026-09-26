---
title: "M17 Production Desktop Hardening"
tags: ["enterprise-local-agent", "m17", "desktop", "hardening", "compatibility", "soak"]
created: 2026-09-26
updated: 2026-09-26
sources: ["README.md", "docs/reports/reports_impl.md", "git history"]
links: ["enterprise-local-agent-milestone-index.md", "m16-desktop-foundation.md", "m14-deployment-operations-hardening.md", "m11-governed-mcp-integration.md", "m6-1-production-linux-localwrite-containment.md"]
category: architecture
confidence: high
schemaVersion: 1
---

# M17 Production Desktop Hardening

## Status

Implemented and committed as `7777c1f`. Closure verdict: **M17 PASS WITH RC
ITEMS**. No M17 tag has been created.

M17 adds no agent capability. It hardens compatibility, lifecycle recovery,
hostile-content handling, process cleanup evidence, bounded soak coverage, and
packaged Linux validation while preserving the M0-M16 authority boundaries.

## Compatibility Boundary

`CompatibilityHandshakeV1` contains only:

- handshake and service API versions;
- supported `ServiceEvent` versions;
- required durable-waiting and owner-authorized-history contract versions;
- a deterministic compatibility-contract fingerprint;
- an opaque service generation.

Build, application version, and git metadata remain informational and do not
change the compatibility fingerprint. The Tauri Rust bridge validates the
strict handshake before every runtime command. A missing endpoint is
`LegacyUnsupported`; only health, version, and compatibility diagnostics remain
available, and JavaScript cannot bypass the Rust-side gate. Unknown or
incompatible schemas fail closed without coercion.

## Lifecycle and Multi-Instance Behavior

Wake, focus, reconnect, and service-generation changes refresh compatibility,
health, readiness, durable session history, selected run status, and event
cursor state. A changed generation clears volatile conversation assumptions and
reloads service-authoritative state. It does not create a run, replay a model
call, repeat retrieval, submit approval, resume, or invoke a tool.

Polling is adaptive rather than permanently fixed: active foreground runs use a
responsive cadence, Waiting runs and background windows slow down, reconnect
refreshes immediately, and terminal runs stop. Cursor validation continues to
ignore duplicates and reject gaps, out-of-order pages, wrong runs, and cursors
ahead of the authoritative sequence.

Multiple desktops remain observers and requesters. The service continues to
own one active run per session, idempotent starts, decision CAS, resume
ownership, cancellation, durable recovery, and manual reconciliation. Closing a
desktop never implicitly cancels a service run.

## Security and Supply Chain

The WebView remains local with strict CSP and named Tauri commands only. It has
no generic HTTP, filesystem, shell, opener, updater, SQL, MCP, model,
persistence, containment, or credential capability. Hostile-content tests keep
JavaScript/data/file links inert, omit remote or local images, escape raw HTML,
and bound Markdown, tables, and code. Clipboard operations remain explicit user
actions.

The release validator checks exact npm dependency versions, registry integrity,
Cargo registry sources, CSP, and capability inventory. `rustls` was upgraded
from `0.23.43` to patched `0.23.45`. npm audit and RustSec report no known
vulnerabilities; transitive maintenance and license inventories are recorded
for release review.

## M11 Cleanup Investigation

The tracked `termination_reaps_process_group_descendant` failure was a test
classification defect, not a live-process leak. The baseline test required
`/proc/<pid>` disappearance. In containers without an init process, the killed
descendant can remain as a PID-1-owned `Z` zombie with the same process start
time, PPID 1, and original PGID/session. With `--init`, it is reaped and
disappears.

The test now records PID, PPID, PGID, session, state, and start time; rejects a
live matching process; detects PID reuse; and tolerates disappearance between
`/proc` observations. Runtime MCP cleanup code is unchanged. Eight host, eight
container-PID1, and eight `--init` cycles passed. The invariant is precise:
`terminate_and_reap` leaves no **live** owned descendant.

## Soak and Performance Evidence

Offline closure runs covered:

- 24 seeded start/cancel cycles;
- five reconnect/wake and cursor-catch-up rounds;
- five each of approve/restart/resume, deny, abort/stale decision, completed
  restart, unresolved-running classification, and durable-Waiting recovery;
- five rounds each of model, knowledge, and citation failure;
- SQLite transaction fault and full ten-test integrity/recovery coverage;
- 24 MCP cleanup environment cycles.

Observed duplicate dispatches, unauthorized mutations, event-continuity
failures, and live surviving owned processes were all zero. Five packaged
close/reopen cycles held file descriptors at 20. RSS samples were 154344,
159772, 159592, 159220, and 160132 KiB; the four warm samples varied by less
than 1 MiB. A 100-session desktop summary rendered in 253 ms under jsdom, and
the debug service exercised the configured 256-session maximum in 1.945 s.
These are baselines, not release budgets.

## Platform and Release Gates

Linux x86_64 passed Debian build and package verification, X11/Xvfb
unavailable-service startup, five close/reopen cycles, Unix-socket and
authenticated-loopback boundary tests, strict workspace Clippy, 333 workspace
tests plus four compile-fail doctests, and the non-skipping M6.1 certification.
The verified Debian artifact SHA-256 was
`fa4b2347abd740c0444144b0b60f98bc392c573988e5392af35a71ff324f3186`.

Native Wayland and physical Linux suspend/wake remain release-candidate items.
Apple Silicon `.app`/DMG production, hardened-runtime validation, native service
connection, close/reopen, and sleep/wake are pending an Apple environment and
are not claimed. A dirty-source manifest verified during development, but only
a clean-source manifest may be published.

## Forward Constraints

- Compatibility must be validated in Rust before runtime commands.
- Missing or unknown compatibility contracts fail closed.
- Service-generation changes reload authoritative state without dispatch.
- No reconnect, wake, observer, or retry path may duplicate an external effect.
- The M11 release invariant is no live owned descendant; zombies and PID reuse
  must be classified explicitly rather than hidden or treated as live.
- Platform-specific validation remains separate: desktop packaging does not
  imply Linux LocalWrite containment or untested macOS guarantees.
- Performance optimization requires repeatable native evidence; bundle size,
  virtualization, and polling must not be changed solely to satisfy warnings.
