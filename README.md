# Enterprise Local Agent

Local-first Rust agent runtime with an enterprise execution harness, fixed-loop
and graph orchestration, replaceable model/provider adapters, governed
model-proposed actions, durable recovery, enterprise knowledge integration,
bounded human-in-the-loop approval, authenticated local service access,
hardened deployment operations, and a thin Tauri desktop client.

## M5 action planning

The Plan phase treats model output as untrusted data. It accepts one strict
provider-neutral JSON envelope, assigns a runtime `ActionProposalId`, validates
the exact tool name and arguments against the registered Draft 2020-12 schema,
and produces a single-use `ValidatedAction`. The Act phase then binds a fresh
`ToolCallId` and delegates policy, budget, audit, and execution to
`ExecutionHarness`.

A proposal is not a `ToolCall` and does not authorize execution. Rig/OpenAI
tool execution, native provider tool calls, retries, and heuristic JSON or
markdown extraction are not enabled.

## M6 approval and containment boundary

M6 keeps capability policy, explicit approval, and technical containment as
independent authorities. ReadOnly actions remain executable without approval.
LocalWrite actions require `M6ApprovalPolicy::RequiresApproval`, a matching
approval decision for the exact validated action, required security audit
records, and a separately registered `ContainedToolPort`. ExternalWrite and
Privileged actions remain non-executable even if a custom policy is faulty.

`ContainedToolPort` is a trusted adapter contract; implementing the Rust trait
alone does not prove operating-system isolation. M6.1 adds the Linux-only
`LinuxWorkspaceWriteTool` for one operation: `workspace_write_file`. It is
usable only after its production capability probe verifies a non-setuid
bubblewrap with FD binding, the self-contained worker, namespaces, no external
network, descriptor/environment isolation, and mandatory `openat2()` flags.
Unsupported hosts fail closed before tool registration.

The worker receives only a final basename and at most 4 KiB of approved UTF-8
content. The trusted parent resolves the target parent beneath the configured
workspace FD and exposes only that directory as writable. Landlock is reported
and used as optional defense-in-depth; it is not part of the mandatory M6.1
availability decision.

The first certified target requires an immutable, statically linked Linux ELF
worker (executable with no write bits, for example mode `0555`). Run the
fail-closed host certification explicitly after installing bubblewrap 0.11.2+
and that worker artifact:

```bash
ELA_M6_1_BWRAP=/usr/bin/bwrap \
ELA_M6_1_WORKER=/absolute/path/enterprise-local-write-worker \
CARGO_HOME=/tmp/enterprise-local-agent-cargo \
cargo test -p agent-containment-linux --all-features \
  --test linux_certification -- --ignored --exact production_linux_security_certification
```

The probe tests required behavior and does not trust the version string alone.
Invoking certification on an unsupported host is a test failure, not a skip.

## M7 durable recovery

M7 journals metadata-only events and quiescent checkpoints through the
provider-neutral `RunPersistencePort`. The SQLite adapter remains outside the
harness. Recovery validates identity, versions, sequence continuity, checksums,
and the event-chain digest, then replays the same deterministic reducer used at
runtime. Replay never invokes model, approval, tool, containment, or other
external ports.

## M8 enterprise knowledge integration

M8 integrates the existing OCPP RAG/KAG and standards-mcp backends behind the
provider-neutral `KnowledgePort`. It does not add ingestion, embeddings,
indexing, or vector storage. Trusted configuration selects one backend or a
deterministic federation; model text cannot select endpoints, processes, or MCP
operations.

The OCPP adapter uses its structured read-only `GET /search` API. The inspected
OCPP MCP tool returns Markdown-only results and does not provide reliable
structured error signaling. The standards adapter uses only the fixed
`search_standards_kag` MCP operation over bounded stdio JSON-RPC. Evidence is
bounded, provenance-preserving, and explicitly supplied to models as untrusted
data. It receives no policy, approval, tool, or containment authority.

M8 recovery persists only retrieval correlation, route, query digest/size,
evidence references/counts, available snapshot metadata, truncation/degradation,
and a manifest digest. Replay is inert. An explicitly retrieval-restartable
program may reconstruct the same query and route and perform a new read after
restart; without a backend snapshot this is fresh retrieval, not historical
replay.

## M9 deterministic graph engine PoC

M9 adds `agent-graph` as a provider-neutral orchestration sibling of
`agent-loop`. It accepts only an immutable validated DAG with typed nodes and
transitions. Graph callbacks receive narrow capability-specific facades that
delegate to `ExecutionHarness`; graph code has no direct access to model,
knowledge, tool, approval, containment, policy, registry, provider, or MCP
ports.

Graph routing is deterministic for a typed callback outcome. Application
callbacks are trusted in-process code and are not assumed pure or deterministic.
Every node consumes an independent `GraphSteps` budget reservation before its
callback, with an implementation ceiling of 64 and a default budget of zero.
The PoC flow is `Retrieve -> Model -> Action -> Verify`, ending at `Complete` or
`Fail`; it reuses M8 retrieval and the existing M5/M6/M6.1 action path.

M9 introduced metadata-only graph position, attempt IDs, transitions, program
version, and graph digest in event schema 8 and checkpoint schema 3. M10
advances the current versions to event schema 9 and checkpoint schema 4. Replay
never invokes graph callbacks or external ports. Retrieval may restart only as
a new fresh read under the explicit restart contract. Model, Action, and Verify
state is never reconstructed; unresolved external effects retain M7 manual-
reconciliation semantics.

## M10 durable graph pause/resume and HITL

M10 adds intentional quiescent `RecoveryDisposition::Waiting`, distinct from
both resumable computation and an unresolved external effect. Durable approval
is an explicit trusted graph/workflow configuration for only the existing
bounded `workspace_write_file` LocalWrite. Model output cannot select this
mode, and immediate M6 `ApprovalPort` behavior is unchanged.

Before a wait becomes visible, the harness validates the exact action, reserves
one ApprovalRequests unit, records the required ApprovalRequested audit, seals
the action, and atomically persists `GraphSuspended` with a CAS-versioned
approval row. SQLite stores only ciphertext and bounded correlation metadata;
events, checkpoints, and audit records contain no action path or content.

`LocalWriteActionCapsuleV1` is encrypted by the harness-owned `ActionSealPort`.
The local adapter uses XChaCha20-Poly1305 and authenticates the run/session,
graph version and digest, node attempt and wait, approval/proposal/tool IDs,
`ActionDigest`, `WorkspaceBindingId`, derived `ToolContractDigest`, and capsule
version. Deployment supplies key material outside SQLite.

After restart, an explicit approval-view operation decrypts and validates the
capsule and regenerates a trusted preview containing only the operation,
relative target, and content byte count. Recording Approve or Deny is a
persistence-only CAS operation and never executes a tool.

Explicit approval resume constructs a fresh `RunContext`, revalidates every
binding, performs exact tool lookup and schema validation, recomputes the action
digest, and rechecks policy, required audit health, containment, and budgets.
It then dispatches the exact action through the existing M6/M6.1 path. Resume
does not invoke the model, retrieval, planning, or the graph Action callback,
and it does not consume another ApprovalRequest or GraphStep. Denial consumes
zero ToolCalls.

Replay never decrypts capsules. `ToolInvocationStarted` without a trustworthy
terminal event remains `ManualReconciliationRequired`. A completed write that
crashes before graph continuation is never repeated; if required transient
state was lost, recovery remains manual/non-resumable. No exactly-once or
audit/SQLite cross-system transaction guarantee is claimed.

## M11 governed MCP integration

M11 adds `agent-mcp-adapters` for governed executable MCP tools. MCP remains
transport and discovery only. Trusted configuration fixes the server ID,
absolute executable, argv, explicit environment, allowlisted remote tool,
trusted local name and description, capability, and expected definition
fingerprint. Model output cannot select an MCP server, endpoint, executable, or
operation.

M11 supports only bounded newline-delimited UTF-8 JSON-RPC over stdio and pins
MCP revision `2025-06-18`, which was negotiated by both configured enterprise
servers during implementation. Discovery performs bounded `tools/list`
pagination, validates the exact allowlist and existing M5 schema profile, and
compares a deterministic SHA-256 definition fingerprint. Every invocation uses
a fresh process and repeats the protocol, definition, and fingerprint checks
before `tools/call`; definition drift dispatches no remote tool call.

Executable MCP operations still follow `ToolDefinition -> ActionProposal ->
ValidatedAction -> ExecutionHarness -> policy/budget/audit -> ManagedToolPort`.
Only trusted-configured ReadOnly tools can dispatch. MCP LocalWrite cannot call
the server and remains containment-unavailable; ExternalWrite and Privileged
remain denied. Existing M8 knowledge retrieval and M6/M6.1 LocalWrite
governance are unchanged.

`ManagedToolInvocation` gives the harness explicit process lifecycle ownership.
The harness durably records `ToolInvocationStarted` before spawning the managed
invocation, and cancellation or deadline terminates and reaps its process group.
An unresolved call recovers as `ManualReconciliationRequired`; replay performs
no MCP spawn, discovery, or invocation.

M11 accepts bounded text and structured JSON results. MCP `isError` maps to a
domain failure, while protocol, process, framing, correlation, timeout, and
drift failures remain sanitized infrastructure failures. Rich content and tools
advertising `outputSchema` are rejected in this milestone. No result content is
stored in AgentEvent or audit records.

A configured MCP executable is trusted infrastructure. ReadOnly mapping limits
what the runtime requests but does not sandbox the executable or prove it
cannot misbehave. M11 provides no HTTP MCP transport, retry/reconnect loop,
dynamic installation, output-schema support, or control over independently
daemonized descendants.

## M12 agent service API

M12 adds a provider-neutral application boundary for local clients:

```text
client
  -> agent-service-http
  -> agent-service
  -> trusted graph or loop workflow
  -> ExecutionHarness
```

`agent-service` provides session creation, idempotent run start, status and
event reads, active-run cancellation, durable Waiting discovery and preview,
Approve/Deny submission, explicit resume, and durable Waiting abort. A trusted
server-side `WorkflowId` selects a fixed composition. Clients cannot choose a
model, tool, MCP server, endpoint, policy, approval implementation, or
containment adapter.

The passive `RunReadPort` exposes bounded run summaries, verified event pages,
and Waiting records without exposing SQLite or granting mutation authority.
`ServiceEventV1` is a stable metadata-only projection rather than a serialized
`AgentEvent`; it contains no prompts, arguments, evidence, tool results,
approval previews, capsules, ciphertext, credentials, or audit internals.

M12 transport is HTTP/JSON plus SSE. The daemon listens on a mode-0600 Unix
socket by default. Optional TCP is restricted to loopback and requires bearer
authentication plus exact Host and Origin allowlists; wildcard CORS and
non-loopback listeners are rejected. Request bodies, run inputs, active runs,
concurrent commands, event pages, and SSE connections are bounded. SSE uses
`EventSequence` as its cursor, reads durable catch-up pages before polling for
new events, and disconnects slow clients instead of blocking runtime execution.
A client disconnect never cancels a run.

`RunSupervisor` permits one active run per session, reserves capacity before
spawn, and uses deterministic run IDs derived from a client start-request ID so
transport retries cannot create a second run. Startup acquires exclusive data-
directory ownership, discovers durable runs without invoking external ports or
decrypting capsules, and exposes their exact M7/M10 recovery dispositions.

Approval clients submit only the wait ID, expected row version, and Approve or
Deny. The harness loads and verifies all M10 security bindings internally.
Aborting a Waiting run atomically terminalizes it as Cancelled, consumes the
wait, executes zero tools, and causes later stale decisions to fail closed.
Persistence remains separate from required M6 audit.

M13 replaces the health-only composition with reviewed enterprise engineering
workflows while retaining `service-health`. Future ACP and Tauri/IDE adapters
must reuse `agent-service` and may not acquire runtime authority.

## M13 local enterprise agent MVP

M13 composes the first real application path from the service boundary through
enterprise retrieval, a trusted-configured OpenAI-compatible model, strict
typed output handling, and the existing governed action pipeline. The graph is
immutable and server-selected; neither clients nor model output can select the
provider, knowledge backend, workflow, policy, tool, or containment adapter.

Two workflow IDs are registered:

- `enterprise-engineering-readonly-v1`: `Retrieve -> Model -> VerifyAnswer ->
  Complete`. It returns a bounded `ApplicationResultV1::FinalAnswer` and does
  not depend on Linux containment.
- `enterprise-engineering-localwrite-v1`: `Retrieve -> Model -> Decision`, then
  either the final-answer path or M5 validation followed by M10 durable
  approval, explicit resume, M6 policy/audit, and M6.1 contained LocalWrite.
  It is registered only when the complete LocalWrite readiness probe succeeds.

Model output must be exactly one JSON object: either `final_answer` plus up to
eight current-evidence citation IDs, or the existing M5 `action` envelope. The
original completed model invocation passes unchanged to M5 for the Action
branch. Mixed responses, extra fields, prose, fences, duplicate keys, malformed
JSON, oversized answers, and unknown citations fail closed. Retrieved evidence
is rendered as untrusted data with provider-neutral evidence IDs; model-supplied
citation metadata is never trusted.

Trusted daemon configuration supplies the opaque model ID, provider label,
endpoint, explicit bearer or loopback no-auth mode, and fixed knowledge route.
No-auth is rejected for non-loopback endpoints. The ReadOnly workflow requires
model, knowledge, persistence, audit, and graph readiness. LocalWrite additionally
requires a stable workspace binding, action-seal key, trusted static worker, and
successful Bubblewrap/openat2 containment probe. LocalWrite readiness failure
does not disable the otherwise healthy ReadOnly workflow and never causes a
silent workflow downgrade.

The explicit real-environment smoke suite has passed against a local
`llama-server` OpenAI-compatible Qwen deployment and the standards knowledge
backend. It verified a cited ReadOnly answer, durable denied LocalWrite with
zero tool dispatch, and approved LocalWrite across service reconstruction with
one observed contained dispatch into a disposable workspace. The M6.1 suite was
also rerun successfully with Bubblewrap 0.11.2 and the static worker. These are
observed test results, not an exactly-once execution claim. Real smoke tests
remain ignored in normal workspace tests and require explicit environment
configuration and invocation.

## M14 deployment and operations hardening

M14 adds `agent-deployment` as the composition and operations layer above the
service. It does not acquire model, tool, approval, policy, containment, or
recovery authority from `ExecutionHarness`.

The daemon now requires an absolute `ELA_DEPLOYMENT_CONFIG` path naming a
strict, versioned `DeploymentConfigV2` TOML document. Unknown fields and
unbounded values are rejected. Configuration covers listener security,
persistence and audit paths, workflow profiles, model and knowledge adapters,
MCP fingerprints, LocalWrite workspace and artifact bindings, external secret
references, and operational limits. Secret values remain outside the document.
A deterministic SHA-256 deployment fingerprint covers security-relevant
non-secret configuration. See
[`docs/deployment-config-v2.example.toml`](docs/deployment-config-v2.example.toml).

ReadOnly and LocalWrite readiness are evaluated independently. Startup performs
bounded deep checks for the dependencies used by configured workflows, then
publishes cached metadata-only readiness. `/healthz` reports process lifecycle
only. LocalWrite registration additionally verifies the seal key reference,
stable workspace binding, derived tool-contract digest, configured Bubblewrap
and worker hashes, root ownership and immutable executable modes, worker
protocol, and the existing bounded M6.1 capability probe. This runtime probe is
not a replacement for release certification. Failed LocalWrite readiness never
silently becomes ReadOnly and does not remove a healthy ReadOnly workflow.

The service has an explicit `Draining` lifecycle. Shutdown stops accepting new
work, requests cancellation through existing harness handles, waits for tracked
tasks within the configured deadline, flushes required audit output, and then
releases process ownership. Durable Waiting remains quiescent. If shutdown
intersects an external effect with no trustworthy terminal record, existing M7
manual-reconciliation semantics remain authoritative.

Operational endpoints expose only bounded readiness, build/version,
fixed-bucket timings and counters, and metadata-only run/reconciliation views.
They never expose prompts, answers, evidence, action paths or content, tool
results, credentials, audit payloads, or sealed capsules. Run IDs are not used
as metric labels.

`SqliteStoreAdmin` provides offline/quiesced backup using SQLite's backup API;
live database files are never copied directly. The backup manifest hashes the
database and audit file and records compatibility-critical store, event,
checkpoint, workflow, graph, capsule, tool-contract, key, workspace, MCP, and
containment identities separately from informational build identity. Restore
requires an empty target and rejects corruption, tampering, unsupported
versions, or incompatible security contracts. No automatic migration,
downgrade, distributed coordination, or cryptographic backup-authenticity claim
is made.

The trusted `agent-operator` CLI is intentionally operational only:

```bash
agent-operator --config /absolute/deployment.toml config validate
agent-operator --config /absolute/deployment.toml readiness
agent-operator --config /absolute/deployment.toml runs list
agent-operator --config /absolute/deployment.toml reconciliation list
agent-operator --config /absolute/deployment.toml backup create /absolute/backup
agent-operator --config /absolute/deployment.toml backup verify /absolute/backup
agent-operator --config /absolute/deployment.toml restore verify /absolute/backup
agent-operator --config /absolute/deployment.toml restore apply /absolute/backup
agent-operator --config /absolute/deployment.toml version
```

It cannot approve, resume, mutate policy, select infrastructure, or access
runtime ports. Online commands use the configured Unix socket or authenticated
loopback listener; backup and restore require exclusive data-directory
ownership.

## M15 enterprise identity and authorization

M15 adds `agent-identity` as a provider-neutral service authorization boundary.
Authentication remains in transport/deployment adapters: Unix sockets map
trusted `SO_PEERCRED` UID/GID pairs to configured principals, while loopback
bearer mode maps one configured credential to one fixed service principal.
Root, PID, request headers, and client-supplied IDs confer no implicit role.

Every client-facing service command receives a `VerifiedPrincipal` and applies
a default-deny `ServiceAuthorizationPolicy`. User, Approver, and Operator roles
are distinct. Run ownership controls status, events, cancellation, abort, and
resume; configured approvers may inspect and decide eligible waits without
owning the run. Optional `RequesterMustDiffer` separation prevents a requester
from approving their own action. Operator access is inspection-only and grants
no workflow, approval, model, tool, or execution authority.

Session ownership and run owner/requester, workflow, authorization-policy
version, and policy fingerprint are durable trusted metadata. Approval
decisions durably record the actor and timestamp while preserving the existing
M10 exact `ActionProposalId + ToolCallId + ActionDigest` binding. The requester
is also authenticated in LocalWrite capsule AAD. Legacy M14 records are never
assigned inferred owners; unowned Waiting state is operator-only and exposed as
`ManualReconciliationRequired`.

Security mutations use required metadata-only audit phases:
`AuthorizationGranted -> MutationRequested -> durable mutation ->
MutationCommitted/Failed`. A required pre-mutation audit failure causes zero
mutation. This service authorization answers who may request an operation; M6
`CapabilityPolicy` remains independently authoritative over whether an exact
validated action may execute.

M15 advances deployment configuration to schema 2 and the explicit
`DeploymentConfigV2` type. See
[`docs/deployment-config-v2.example.toml`](docs/deployment-config-v2.example.toml).
No tokens, credentials, emails, claims blobs, action payloads, or unnecessary
PII enter durable identity records, events, logs, or debug output.

## M16 desktop foundation and app shell

M16 adds `apps/agent-desktop`, a Tauri 2 desktop client over the existing
service boundary. The frontend uses React 19, TypeScript, Vite, Tailwind CSS 4,
shadcn-compatible primitives, TanStack Query, Lucide, and restrained Motion
transitions. It is a thin client and acquires no model, tool, approval, policy,
persistence, MCP, containment, or execution authority.

The Tauri Rust layer owns `LocalServiceClient`. It connects through the local
Unix socket where supported or an explicitly configured authenticated numeric-
loopback fallback. Transport credentials never enter JavaScript. The WebView
can invoke only named typed commands for service health, cached readiness,
build/version information, and the reviewed ReadOnly and LocalWrite approval
lifecycles.
It has no filesystem, shell, generic HTTP, SQL, model, MCP, persistence, or
containment capability, and native window decorations remain enabled.

M16.2 provides a conversation-first application shell with a compact top bar,
collapsible session sidebar, central task workspace, composer surface, optional
readiness inspector, command palette, keyboard shortcuts, and a subtle
connection footer. It supports semantic light, dark, and system themes, visible
focus states, reduced motion, and compact windows from `760x520`. Operational
information remains secondary in the inspector.

M16.3 wires the existing `enterprise-engineering-readonly-v1` workflow into a
real conversation UX. The typed bridge can create a session, start the fixed
ReadOnly run, read status and bounded `ServiceEventV2` pages, cancel an active
run, and retrieve the terminal `ApplicationResultV1`. TanStack Query owns
in-memory server state and cursor catch-up; prompts and answers are never stored
in browser storage. Compact progress exposes only knowledge search, model,
verification, finishing, reconnection, and cancellation states.

Final answers render bounded Markdown with copy controls. Citation chips use
trusted provenance returned by M13 rather than model-supplied citation
metadata. Retry always starts a new run and never replays an old model call.
The UI presents sanitized service, model, knowledge, malformed-result,
cancellation, and volatile-result-loss states without exposing raw prompts,
evidence, model output, internal events, credentials, or action data.

M16.3 does not add LocalWrite, durable approval/HITL, settings, or desktop
payload persistence. Event delivery uses bounded cursor polling over named
commands rather than exposing generic HTTP or SSE transport to the WebView.
`agent-service` and `ExecutionHarness` remain authoritative.

M16.4 adds compact agent activity and an on-demand run inspector without
turning the conversation into an operations dashboard. Active runs show only
safe user-facing phases, elapsed time, and Stop; terminal runs collapse to a
small outcome, duration, and citation-source summary. The inspector remains
closed by default and derives workflow, phase, model-call count, model and
graph budgets, retrieval backends, citations, correlation IDs, and a bounded
Retrieve/Model/Verify/Complete timeline exclusively from `RunView` and
`ServiceEventV2` metadata. It never renders internal event names, prompts,
evidence, model output, reasoning, action arguments, or tool results.

Cursor handling now accepts the service's zero-based `EventSequence`, ignores
duplicates, and rejects gaps, out-of-order pages, or wrong-run events without
advancing the cursor. Terminal status wins permanently over transient
activity. Markdown/GFM and the inspector are lazy-loaded; the production build
emits separate approximately 155 KiB and 6 KiB chunks and reduces the initial
JavaScript chunk to approximately 544 KiB. Evidence count is shown as
`Not exposed` because the current `ServiceEventV2` does not carry it; the UI
does not substitute citation count or infer unavailable metadata.

M16.5 exposes the existing M10/M15 durable LocalWrite flow without changing
its authority. The desktop may start only the fixed
`enterprise-engineering-localwrite-v1` workflow, discover durable Waiting
records, request the trusted bounded approval preview, submit Approve or Deny,
explicitly resume a decided run, or abort a Waiting run. The inline approval
surface displays only operation, workspace-relative target, and content byte
count. It never displays file content, raw model output, action arguments,
capsules, `ActionDigest`, or internal tool bindings.

Approve and Deny use the explicit `approval_submit_decision` Tauri command.
JavaScript supplies only session/run/wait addressing, expected row version,
and the public decision; the Rust bridge maps that to the fixed M15 service
approval endpoint. All exact M10 action bindings, identity authorization, CAS
validation, policy, required audit, and M6.1 containment remain server-side.
Approval does not execute automatically: the user must explicitly resume the
same durable run. Deny and abort execute no tool, stale or unauthorized
decisions fail closed, and restart reconstruction comes from service state
rather than browser storage.

M16.6 replaces process-local sidebar history with service-authoritative durable
conversation metadata. Two named Tauri commands list owner-authorized sessions
and bounded run pages through provider-neutral read ports; SQLite remains behind
`agent-persistence-sqlite` and is never exposed to the desktop. The sidebar
loads compact summaries first, then loads run metadata and events only for the
selected conversation. Waiting, failed, completed, and reconciliation states
therefore survive desktop and service restart without replaying model,
knowledge, approval, tool, or containment effects.

Conversation payloads still are not stored in the WebView. If a volatile M13
terminal result remains available from the service, it is rendered as the
authoritative answer. After service restart, a missing result is shown exactly
as `Result unavailable`; the desktop never regenerates it. Previous run status
metadata is available on demand in the inspector. Rename and archive remain
deferred, and the only browser-stored value remains the theme preference.

M16.7 adds a lazy-loaded Settings surface for local appearance preferences and
safe runtime status. It shows bounded service, model, knowledge, workflow,
LocalWrite, containment, identity, version, budget, and authorized
reconciliation projections. Settings is observational: it cannot modify
deployment configuration, endpoints, policy, identity roles, seal keys,
containment, or runtime authority. Operator-only information remains protected
by the existing M15 authorization boundary.

M16.8 completes the desktop quality pass across typography, Markdown, code,
tables, citations, activity, HITL, composer behavior, focus handling, reduced
motion, and compact layouts. The composer auto-grows within a bounded height,
manual scrolling exposes `Jump to latest`, cancellation is visually distinct
from failure, and manual reconciliation remains explicitly non-resumable.

The native Tauri application was reviewed at `760x520`, approximately
`1280x800`, and `1700x900`, in light and dark themes and at 100%, 150%, and 200%
GTK scaling. That review corrected high-DPI WebView clipping, stale connected
status after a failed health refresh, cancellation styled as failure, and
generic failure text for reconciliation. The native window minimum remains in
Tauri, and the named command and capability boundary is unchanged. M16.8 passed
36 frontend tests, 9 desktop Rust tests, strict Clippy, and the Tauri release
build.

M16.9 adds release packaging without embedding runtime authority. Tauri now
produces an Apple Silicon `.app` and DMG with macOS 12 minimum and hardened
runtime settings, plus an x86_64 Debian package. The desktop version inherits
the workspace Cargo version; npm mirrors are checked, and bounded desktop build
metadata reports the version, git revision, build profile, and supported service
API/event contracts. Production source maps and automatic updater artifacts are
disabled.

Credential-free CI builds unsigned packages. Production macOS signing,
notarization, stapling, and verification use release-operator credentials held
outside the repository. Deterministic manifests record compatibility metadata
and SHA-256 values and reject dirty sources or tampered artifacts. The desktop
continues to require a separately installed trusted `agent-service-daemon`; it
does not bundle or launch the service or containment artifacts. See
[`docs/release/desktop.md`](docs/release/desktop.md) for commands and policy.

The Linux release path produced and verified an installable x86_64 Debian
package, including desktop entry, icons, declared GTK/WebKit runtime closure,
install/uninstall behavior, unavailable-service startup, and package leakage
scans. Release configuration tests, frontend checks, desktop Rust tests, strict
workspace Clippy, and the workspace suite passed. macOS artifact production,
Developer ID signing, notarization, and stapling still require an Apple Silicon
release environment. Initial releases remain manual; automatic updates are
disabled.

## M17 production desktop hardening

M17 hardens the packaged desktop and local-service boundary without adding
agent capability or moving authority out of `ExecutionHarness`. The service now
publishes a strict `CompatibilityHandshakeV1` containing only protocol and
contract versions, a deterministic security-contract fingerprint, and an
opaque service generation. The desktop validates that handshake in Rust before
every runtime command. A pre-M17 service without the endpoint is
`LegacyUnsupported`: health and version diagnostics remain available, while
runtime reads and mutations are blocked with a sanitized upgrade-required
state.

Wake, reconnect, and service-generation changes trigger authoritative health,
readiness, durable history, run-status, and cursor refresh. A generation change
discards only volatile desktop assumptions and reloads durable service state;
it never dispatches or replays an effect. Event polling is bounded and adaptive:
active runs remain responsive, Waiting and background views poll more slowly,
and terminal runs stop polling. Existing M7/M10 recovery and one-active-run,
idempotent-start, approval-CAS, and resume ownership rules remain authoritative
for multiple desktop observers.

The WebView retains its local-only CSP and named-command ACL. Hostile Markdown
tests keep JavaScript, data, and file links inert, omit model-supplied images,
escape raw HTML, and bound large tables and code. Dependency checks upgraded
`rustls` to `0.23.45`; npm and RustSec scans report no known vulnerabilities.
M17 also resolves the tracked M11 cleanup regression as a test-classification
defect: container PID 1 may retain an exited zombie, but repeated host,
container, and `--init` evidence confirms that `terminate_and_reap` leaves no
live owned descendant. Runtime MCP cleanup code did not require a change.

Bounded offline soak covered 24 start/cancel cycles, five reconnect rounds,
repeated Approve/Deny/abort/stale-decision and restart scenarios, model and
knowledge failures, SQLite fault recovery, and 24 MCP cleanup environment
cycles. It observed zero duplicate dispatches, unauthorized mutations, event
continuity failures, or live surviving descendants. Five packaged Linux
close/reopen cycles held 20 file descriptors and stable warm-run memory.

M17 is committed as `7777c1f` and closes as **PASS WITH RC ITEMS**. The Linux
x86_64 Debian package, X11 startup, full workspace suite, strict Clippy, M11
cleanup, and non-skipping M6.1 certification pass. Release-candidate validation
still requires a clean-source manifest, native Wayland and physical
suspend/wake checks, organizational dependency-license approval, and native
Apple Silicon package, service-connection, close/reopen, and sleep/wake
validation. Untested macOS behavior is not claimed.

## Deterministic demonstration

The default CLI path is network-free. It uses `RigModelAdapter<FakeRigModel>`
to return a strict action envelope, then validates and executes the
deterministic ReadOnly fake tool through the harness:

```bash
cargo run -p agent-cli
```

It prints only run identifiers, provider label, loop progression, final status,
budget usage, and audit-degraded status.

An explicit governance-only demonstration is also available:

```bash
cargo run -p agent-cli -- --demo-local-write-fake-containment
```

That mode uses scripted fake approval and a test fake contained executor. It
does not write files, launch processes, use a network, or provide OS isolation;
the CLI labels both fakes prominently.

## OpenAI-compatible live mode

Live provider access is explicit and is never selected by default:

```bash
ELA_OPENAI_COMPAT_BASE_URL=http://127.0.0.1:8000/v1 \
ELA_OPENAI_COMPAT_MODEL=your-deployed-model-identifier \
ELA_OPENAI_COMPAT_API_KEY=your-bearer-credential \
ELA_OPENAI_COMPAT_LABEL=local-qwen \
cargo run -p agent-cli -- --live-openai-compatible
```

The model identifier is opaque configuration. It may be a concrete deployment,
an enterprise-gateway alias, or a virtual-model identifier.

HTTP is accepted only for `localhost` or an IPv4/IPv6 loopback address. All
other endpoints require HTTPS. The base URL must be an API root such as `/v1`,
not `/v1/chat/completions`. Bearer authentication is mandatory in M4.

The CLI does not print prompts, model responses, credentials, full endpoint
URLs, or model identifiers. Production compositions must not enable TRACE-level
Rig/provider logging without first reviewing the exact pinned Rig version for
content leakage.

The live mode uses the same M5 `ActionProgram`; only model composition changes.
If a live model returns prose, code fences, malformed JSON, or an invalid
action, preparation fails closed without extraction heuristics.

The M4-M6.1 layers themselves intentionally do not provide readiness calls,
retries, proxy settings, custom certificate authorities, mTLS, streaming,
structured output, model fallback, native provider tool calls, provider-driven
tool execution, enterprise identity, or durable approval. M10 adds durable
approval only for its explicitly configured `workspace_write_file` graph node;
it is not a generic approval queue or workflow engine. M6.1 containment remains
Linux-only and is not an arbitrary tool sandbox. M11 enables only configured
ReadOnly MCP tool requests and does not make MCP a policy, approval,
containment, or execution authority.
