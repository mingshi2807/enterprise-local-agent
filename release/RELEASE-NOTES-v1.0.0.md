# Enterprise Local Agent 1.0.0

Enterprise Local Agent 1.0.0 is the first production release for Linux x86_64.
It promotes the M18 release candidate and the subsequently validated end-user
fixes from `main` without changing the runtime architecture.

## Highlights

- Local-first enterprise execution harness with bounded budgets, cancellation,
  audit, provider-neutral model access, and strict typed action validation.
- Enterprise knowledge retrieval through the reviewed Standards and OCPP
  adapters, with bounded evidence and trusted citation provenance.
- Durable SQLite journal, checkpoint/recovery classification, deterministic
  graph workflows, and durable human approval for LocalWrite.
- Linux `workspace_write_file` containment using a separate static worker,
  Bubblewrap FD binding, `openat2()` path resolution, no external network, and
  required policy, approval, and audit gates.
- Authenticated service API with durable ownership, authorization, metadata-only
  events, restart-safe Waiting approvals, and operator diagnostics.
- Tauri desktop with conversations, citations, run activity, approval UX,
  durable history, settings, compatibility validation, and Linux packaging.

## Validated Deployment

The release path was validated with a real local Qwen OpenAI-compatible model,
the Standards knowledge backend, packaged desktop, durable service state, and
the certified Linux LocalWrite environment. The acceptance scenario observed
exactly one contained dispatch after durable approval and explicit restart/
resume. This is not a global exactly-once execution claim.

## Platform and Deployment

- Supported platform: Linux x86_64.
- The Debian package installs the desktop only.
- `agent-service-daemon`, the model endpoint, knowledge backends, and Linux
  containment artifacts are separately deployed trusted infrastructure.
- Automatic desktop updates are disabled for the initial production release.
- macOS is post-v1 and is not advertised as supported.

## Known Limitations

- OCPP retrieval is unavailable while its configured corpus is empty.
- Models that violate the strict structured-output contract fail closed.
- Global exactly-once external-effect execution is not claimed; uncertain
  effects require manual reconciliation and are never automatically retried.
- Real two-principal delegated approval and deliberately induced real
  ManualReconciliationRequired UI acceptance remain deterministic-test-backed.

See [`quickstart.md`](../quickstart.md) for installation, service configuration,
ReadOnly and LocalWrite use, troubleshooting, removal, and the security model.
