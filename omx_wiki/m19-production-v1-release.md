---
title: "M19 Production v1.0 Release"
tags: ["enterprise-local-agent", "m19", "production", "v1.0.0", "linux"]
created: 2026-09-28
updated: 2026-09-28
sources: ["README.md", "quickstart.md", "release/RELEASE-NOTES-v1.0.0.md", "release/THIRD-PARTY-LICENSES.json", "git history"]
links: ["enterprise-local-agent-milestone-index.md", "m18-release-candidate-closure.md", "m17-production-desktop-hardening.md", "m6-1-production-linux-localwrite-containment.md"]
category: architecture
confidence: high
schemaVersion: 1
---

# M19 Production v1.0 Release

## Scope

M19 promotes the validated M18 Linux release candidate and accepted end-user
fixes from `main` to production version `1.0.0`, tagged `v1.0.0`. It adds no
agent capability and changes no execution authority or architecture. The
supported production platform is Linux x86_64; macOS remains post-v1.

## Release Contract

The final package is built from clean tagged source. The release manifest binds
the package to the exact commit, version, target, compatibility fingerprint,
build profile, and `source_dirty=false`. SHA-256 checksums cover the package
and manifest. A deterministic inventory records every third-party Cargo and npm
package and its declared license from the locked dependency graph.

The desktop is packaged separately from `agent-service-daemon`. It does not
embed or launch the model, knowledge backends, service, persistence, audit,
action sealing, Bubblewrap, or containment worker. Runtime authority remains:

```text
Desktop -> agent-service -> model / knowledge -> governed execution
```

## Production Capabilities

The production release includes provider-neutral model and knowledge ports,
strict action validation, policy, budgets, required audit, Linux containment,
durable persistence/recovery, deterministic graph workflows, durable HITL,
MCP ReadOnly governance, authenticated service APIs, deployment operations,
identity/authorization, and the packaged Tauri desktop.

The real acceptance path used local Qwen, Standards knowledge, durable state,
and certified LocalWrite containment. Exactly one contained dispatch was
observed in the approved restart/resume acceptance scenario. Global exactly-once
external-effect execution is not claimed.

## Release Gates

The release gate set comprises frontend typecheck/lint/tests/build, Rust format,
strict Clippy, unfiltered workspace tests, persistence/recovery, M11 process
cleanup, M6.1 non-skipping certification, compatibility and capability checks,
dependency vulnerability and license scans, package verification, clean-source
manifest verification, and Debian install/start/remove validation.

## Known Limitations

- Linux x86_64 is the only v1 supported platform; macOS is post-v1.
- OCPP retrieval is unavailable while its corpus is empty.
- Strict model-output violations fail closed without heuristic parsing.
- Uncertain external effects require manual reconciliation and are not retried.
- Real delegated two-principal approval and deliberately induced real
  ManualReconciliationRequired UI acceptance remain deterministic-test-backed.
- Automatic desktop updates remain disabled.
