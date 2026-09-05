---
title: "M6.1 Production Linux LocalWrite Containment"
tags: ["m6.1", "linux", "bubblewrap", "openat2", "local-write"]
created: 2026-09-04
updated: 2026-09-04
links: ["enterprise-local-agent-milestone-index.md", "m6-approval-and-containment-boundary.md", "m7-durable-event-persistence-and-recovery.md"]
category: architecture
confidence: high
schemaVersion: 1
---

# M6.1 Production Linux LocalWrite Containment

## Status

Implemented. Deterministic verification is complete; production Linux security
certification requires a non-setuid bubblewrap 0.11.2 or newer with functional
`--bind-fd` and `--ro-bind-fd` support, plus an immutable self-contained worker.
The current host bubblewrap 0.6.1 is intentionally unavailable.

## Scope

The only production operation is `workspace_write_file(relative_path, content)`.
It preserves the M6 path from validated action through policy, exact approval,
required audit, and `ContainedToolPort`. Shells, commands, provider tools,
ExternalWrite, Privileged, retries, MCP, RAG, and graphs remain excluded.

## Implementation

`agent-containment-linux` depends inward on `agent-harness` and `agent-core` and
contains `LinuxWorkspaceWriteTool`, a bounded protocol, a bubblewrap launcher,
and `enterprise-local-write-worker`. The constructor probes the actual sandbox
before the tool can be registered.

The trusted parent opens the workspace and worker with CLOEXEC descriptors. It
resolves the approved target parent with `openat2(RESOLVE_BENEATH |
RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS | RESOLVE_NO_XDEV)`, then passes
only that parent FD and the worker FD to bubblewrap. A child-only `pre_exec`
hook uses `close_range(CLOSE_RANGE_CLOEXEC)`, `dup2`, and `fcntl`; it never
changes descriptor flags in the multithreaded parent.

Bubblewrap creates user, mount, PID, network, IPC, and UTS namespaces, drops
capabilities, clears the environment, uses a private tmp, a private proc, an
empty dev, a read-only worker bind, and the target-parent-only writable bind.
The worker sets no-new-privileges and applies Landlock on a best-effort basis.

Systemd service sandboxing is useful deployment defense-in-depth for the
long-lived agent process, particularly resource limits, credential isolation,
and a restricted service filesystem. It is not a substitute for this per-call
bubblewrap policy: it does not provide the approved target-parent FD binding or
the worker's descriptor-relative `openat2()` write boundary.

The worker accepts one versioned length-prefixed request, writes at most 4 KiB
to a mode-0600 temporary file in the target directory, syncs it, atomically
renames it, and syncs the directory. It never follows symlinks. Errors and
responses are bounded and sanitized; content is absent from previews and audit.

## Lifecycle Amendment

`ContainedToolPort` now starts a provider-neutral `ContainedInvocation` handle.
The harness waits on it normally and calls `terminate_and_reap` before returning
from contained cancellation or deadline paths. This preserves M6 ordering,
budgets, policy, digest binding, audit, loop behavior, and provider boundaries.

## Verification Contract

Normal workspace tests are deterministic and host-independent. The ignored
`production_linux_security_certification` test must be invoked explicitly; once
invoked it fails rather than skips when mandatory capabilities are absent. It
checks the production probe, governed create/replace, symlink escape rejection,
functional `NO_XDEV` rejection across the root-to-`/proc` mount boundary,
outside-boundary protection, payload-free audit, and ToolCallId preservation.
The certification feature also exercises killing and reaping bubblewrap, the
worker, and a worker descendant.

The worker used for production/certification must be a statically linked Linux
ELF executable with no write bits (for example mode `0555`). Run certification
with explicit trusted paths:

```bash
ELA_M6_1_BWRAP=/usr/bin/bwrap \
ELA_M6_1_WORKER=/absolute/path/enterprise-local-write-worker \
CARGO_HOME=/tmp/enterprise-local-agent-cargo \
cargo test -p agent-containment-linux --all-features \
  --test linux_certification -- --ignored --exact production_linux_security_certification
```

## Non-guarantees

There is no rollback after rename, no protection from kernel or bubblewrap
vulnerabilities, no defense against a hostile same-UID host process racing the
workspace, no enterprise approval identity, and no general command sandbox.
