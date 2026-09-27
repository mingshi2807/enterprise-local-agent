---
title: "M18 Release Candidate and Real-User Validation"
tags: ["enterprise-local-agent", "m18", "release-candidate", "linux", "acceptance"]
created: 2026-09-27
updated: 2026-09-27
sources: ["README.md", "docs/reports/reports_impl.md", "release/M18-ADMINISTRATIVE-CLOSURE.md", "release/enterprise-local-agent-desktop-linux-x86_64.json", "git history"]
links: ["enterprise-local-agent-milestone-index.md", "m17-production-desktop-hardening.md", "m16-desktop-foundation.md", "m13-local-enterprise-agent-mvp.md", "m6-1-production-linux-localwrite-containment.md"]
category: architecture
confidence: high
schemaVersion: 1
---

# M18 Release Candidate and Real-User Validation

## Status

M18 closes at **PASS** for Linux x86_64. The final release candidate is version
`0.1.0-rc.2`, commit
`69667f626566ad29bafe6ea26ea6c7d9a6cee89e`, tagged
`m18.3-linux-rc2`. M18 adds no product feature or execution authority.

The release manifest records `source_dirty=false`, target `linux-x86_64`, and
compatibility fingerprint
`9244db3dd8e2b578ac5b7424c9c5f3f9c29262310db183ccd5c92ba3c434c578`.
The desktop remains a thin client over a separately deployed trusted service.

## Real Stack and Acceptance

Acceptance used the actual local Qwen OpenAI-compatible endpoint, Standards
MCP knowledge backend, service daemon, packaged Tauri desktop, durable
SQLite/audit, M15 identity, action sealing, and certified M6.1 containment.
The OCPP and federated cases remained not applicable while the OCPP corpus was
empty; no fixture was substituted for RC acceptance.

The real ReadOnly path completed Standards retrieval, constrained model output,
strict final-answer decoding, and trusted citation validation. LocalWrite
covered Deny with zero dispatch and Approve followed by service restart,
explicit Resume, one contained write, and terminal graph continuation. Durable
Waiting survived desktop/service restart, stale decisions failed closed, and
multiple desktops remained observers under service-owned idempotency and CAS.

## Linux Native Lifecycle

The final Debian package passed clean-source build, layout verification,
upgrade from rc.1, native Wayland launch, unavailable-service recovery,
close/reopen, remove, reinstall, and post-reinstall startup. X11/XWayland was
also exercised, but Linux x86_64 rather than a display protocol is the release
scope.

Three physical `s2idle` cases passed:

- idle desktop suspended and resumed with the same service generation;
- an active ReadOnly run resumed, caught up contiguous sequences, invoked the
  model once, and surfaced strict validation failure without stale success;
- durable Waiting resumed with the same WaitId, row version, principal context,
  and sequence, with zero tool starts and no target file.

Wake refresh revalidated compatibility, health, readiness, authoritative run
status, and EventSequence. No duplicate run, model, approval, or tool dispatch
was observed.

## Approval Conflict Correction

Expected stale CAS and already-decided approval conflicts map to HTTP
`409` with stable code `state_conflict`. A first Deny returned `204`; its
duplicate returned `409`, started zero tools, and created no file. This is only
an HTTP error-classification correction. M10 exact-action binding, M15
authorization, wait CAS, approval state, budgets, and containment are unchanged.

## Security and Release Gates

Frontend checks, formatting, strict Clippy, unfiltered workspace tests,
persistence/recovery tests, M11 descendant cleanup, dependency scans, package
verification, and the non-skipping M6.1 certification passed. M11 cleanup still
requires no live owned descendant; PID-1-owned exited zombies are classified
rather than mistaken for live escapes.

The approved inventory is bound to these exact files:

- 575 Rust packages, `Cargo.lock` SHA-256
  `9da1946dc250bc76be269fd2ac04b3697205d250c003a2af317e875f02bd3eab`;
- 547 npm packages, `package-lock.json` SHA-256
  `d6e2364c234cc9403ce6af9b48262e17c8427acd11800df1aeee0ced446a6dfb`.

There are zero missing third-party license declarations and zero known
dependency vulnerabilities. The Project Owner / Release Authority approved
this exact RC inventory on 2026-09-27.

## Durable Release Evidence

Project-local [`release/`](../release/) contains:

- `Enterprise Local Agent Desktop_0.1.0-rc.2_amd64.deb`, SHA-256
  `db2cb9db4dcd88dadbebbb5400491900d041b11870dee36a2e9afb6c0fd1af04`;
- `enterprise-local-agent-desktop-linux-x86_64.json`, SHA-256
  `2b1ad203e697349109dbfb76609e0144d82002f73cda924da852ffcccf9bb20b`;
- the exact approved Cargo and npm lockfiles;
- `M18-ADMINISTRATIVE-CLOSURE.md`;
- `SHA256SUMS`.

The manifest and checksum index verify successfully. The release directory is
evidence and distribution material, not runtime configuration or authority.

## Scope and Forward Constraints

- Linux x86_64 is the v1 platform scope.
- Apple Silicon macOS remains post-v1 and is not advertised as validated.
- Automatic updates remain disabled pending signing and rollback review.
- Release evidence does not replace M6.1 host readiness or certification.
- Reconnect, wake, retry, and multiple observers must never duplicate effects.
- Strict model-output validation remains fail closed; no heuristic parser was
  introduced to make acceptance pass.
- Real two-principal RequesterMustDiffer and deliberately induced
  ManualReconciliationRequired UI states remain deterministic-test-backed
  limitations, not claimed real-environment exercises.
