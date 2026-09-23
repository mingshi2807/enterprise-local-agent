---
title: "M16 Desktop Foundation, Governed Conversation, and Quality"
tags: ["m16", "desktop", "tauri", "react", "design-system", "app-shell", "conversation", "readonly", "activity", "inspector", "hitl", "localwrite", "approval", "history", "recovery", "settings", "accessibility", "native-review"]
created: 2026-09-21
updated: 2026-09-23
sources: ["apps/agent-desktop/src-tauri/src/service_client.rs", "apps/agent-desktop/src-tauri/src/lib.rs", "apps/agent-desktop/src-tauri/capabilities/main.json", "apps/agent-desktop/src/app/App.tsx", "apps/agent-desktop/src/app/CommandPalette.tsx", "apps/agent-desktop/src/bridge/contracts.ts", "apps/agent-desktop/src/bridge/service.ts", "apps/agent-desktop/src/queries/conversation.ts", "apps/agent-desktop/src/features/runActivity.ts", "apps/agent-desktop/src/components/ApprovalPanel.tsx", "apps/agent-desktop/src/components/ConversationWorkspace.tsx", "apps/agent-desktop/src/components/MarkdownAnswer.tsx", "apps/agent-desktop/src/components/RunInspector.tsx", "apps/agent-desktop/src/components/SessionSidebar.tsx", "apps/agent-desktop/src/components/SettingsView.tsx", "apps/agent-desktop/src/styles/tokens.css"]
links: ["enterprise-local-agent-milestone-index.md", "m15-enterprise-identity-authorization.md", "m12-agent-service-api.md", "m14-deployment-operations-hardening.md"]
category: architecture
confidence: high
schemaVersion: 1
---

# M16 Desktop Foundation, Governed Conversation, and Quality

## Status

M16.0 architecture was approved. M16.1 through M16.8 are implemented and
committed. M16.7 is `8584c1d`; the completed M16.8 native quality pass is
`caf5204`. No M16 milestone tag has been created.

## Boundary

The desktop is a thin request-and-observation client:

```text
React WebView
  -> named Tauri commands
  -> LocalServiceClient
  -> Unix socket or authenticated loopback
  -> agent-service-http
  -> agent-service
  -> existing ExecutionHarness authority
```

No model, knowledge, tool, approval, policy, persistence, MCP, containment, or
execution authority moves into the desktop. Conversation operations remain
typed service requests and cannot call runtime ports directly.

## M16.1 Tauri Foundation

`apps/agent-desktop` combines Tauri 2 with React 19, TypeScript, Vite, Tailwind
CSS 4, shadcn-compatible primitives, TanStack Query, Lucide, and Motion.
Semantic light, dark, and system themes use restrained neutral tokens, compact
spacing, limited radii, visible focus states, and native window decorations.

The Rust `LocalServiceClient` supports an absolute Unix-socket path where
available and a configured authenticated numeric-loopback fallback. It rejects
non-loopback endpoints, credentials in URLs, redirects, proxies, oversized
responses, and unknown response fields. Bearer material remains Rust-side and
is redacted from debug output.

The initial WebView capability contained only named `health`, `readiness`, and
`version` commands. M16.3 adds five reviewed conversation commands and M16.5
adds only the reviewed LocalWrite workflow and durable-approval commands,
without adding generic transport. There are no filesystem, shell, generic
HTTP, SQL, model, MCP, persistence, or containment plugins. CSP disables
WebView network connections.

## M16.2 Conversation-First Shell

At M16.2 the desktop established:

- a compact application bar and subtle connection footer;
- a collapsible session sidebar with placeholder layout data;
- a dominant central conversation/task workspace;
- a task-composer surface that did not yet dispatch work;
- an optional readiness inspector for secondary operational metadata;
- an accessible command palette with filtering and keyboard navigation;
- shortcuts for new task, sidebar, inspector, and command palette;
- responsive compact, normal, and wide layouts from a `760x520` minimum.

Motion is limited to pane and palette transitions and respects reduced-motion
preferences. The UI uses 1 px borders, neutral surfaces, restrained accent,
almost no shadow, and no gradients or decorative animation. Compact windows
start with a 48 px conversation rail and a closed inspector so the task
workspace remains usable.

## M16.3 ReadOnly Conversation

The desktop now executes only the reviewed
`enterprise-engineering-readonly-v1` workflow:

```text
prompt
  -> named Tauri command
  -> agent-service session and fixed workflow run
  -> bounded ServiceEventV2 cursor pages
  -> authoritative terminal ApplicationResultV1
  -> Markdown answer plus trusted citations
```

The Tauri bridge exposes exactly five additional commands: create session,
start the fixed ReadOnly run, get run status, cancel the active run, and read
bounded event pages. Rust validates UUID correlation, workflow identity,
response bounds, strict unknown-field rejection, event version and sequence,
and terminal result shape. Payload-bearing result debug output is redacted.

TanStack Query owns volatile conversation and server state. React local state
owns only the composer and temporary interaction state. Prompts, answers, and
citations are not written to `localStorage` or `sessionStorage`; the existing
theme preference is the only browser-stored UI value. Retry creates a new run
with a new start request ID and never replays an old model call.

The workspace provides editable multiline composition, Ctrl/Cmd+Enter send,
one active send per conversation, stop/cancel, bounded Markdown and GFM,
copy-answer and copy-code controls, trusted expandable citation details,
progress projection, reconnect/cursor catch-up, tail-following that yields to
manual scrolling, new conversations, and sanitized failure states.

Only metadata from `ServiceEventV2` drives progress. Raw prompts, evidence,
model output, action/tool internals, credentials, and internal `AgentEvent`
payloads are neither requested nor rendered. Model-generated links are shown
as inert text. The terminal application result, not progress events, is the
answer authority.

## M16.4 Activity and Run Inspector

The conversation remains the default visual focus. While a run is active, one
compact row shows only a user-facing phase (`Searching knowledge`, `Thinking`,
`Verifying`, `Finishing`, reconnecting, or stopping), elapsed time, a restrained
activity indicator, and Stop. After termination it collapses to outcome,
duration, and citation-source count. No internal event name or reasoning trace
is displayed.

The right inspector is closed by default and lazy-loaded on demand. It derives
the following only from bounded `RunView` and `ServiceEventV2` metadata:

- workflow, disposition, current phase, elapsed duration, and terminal status;
- unique model-call count plus available model and graph budget counters;
- retrieval backend route and citation count;
- secondary copyable Run, retrieval, and model correlation IDs;
- a lightweight Retrieve, Model, Verify, Complete timeline.

The current event projection does not expose retrieval evidence count. The
inspector displays `Not exposed` and does not misuse citation count as evidence
count. Prompts, evidence content, raw model output, reasoning, action arguments,
tool results, internal `AgentEvent` structures, and credentials remain absent.

Event-page reduction now accepts the service's zero-based sequence, ignores
already-seen duplicates, and requires exact contiguous ordering for new events.
A gap, out-of-order page, or wrong RunId leaves the cursor unchanged and enters
the existing reconnecting state. Once status is terminal, polling stops and no
later transient activity can replace the terminal summary or result.

Markdown/GFM rendering and the run inspector use natural React lazy boundaries.
The final build emits an approximately 155 KiB Markdown chunk and 5.8 KiB
inspector chunk. The initial minified JavaScript chunk falls from approximately
697 KiB to 543.5 KiB; Vite's advisory remains because it is still above 500 KiB.

## M16.5 Durable LocalWrite Approval

M16.5 renders the existing M10/M15 durable approval lifecycle inside the
conversation without changing policy, approval, persistence, or containment:

```text
fixed LocalWrite workflow -> durable Waiting -> trusted preview
  -> Approve or Deny -> explicit Resume -> existing governed execution
```

The bridge adds named commands only for starting
`enterprise-engineering-localwrite-v1`, listing Waiting records, loading a
trusted preview, submitting a decision, explicitly resuming, and aborting a
Waiting run. Approve and Deny use `approval_submit_decision`; JavaScript passes
only SessionId, RunId, WaitId, expected row version, and `approve | deny`.
`LocalServiceClient` maps this to the fixed M15 approval-decision endpoint with
a strict body containing only expected row version and decision. The Tauri ACL
allows this command explicitly and exposes no generic URL or method facility.

The inline approval panel displays only the trusted operation label,
workspace-relative target, and content byte count. It never receives or renders
file content, action arguments, raw model output, capsule data, ActionDigest,
ToolCallId, or other exact-action bindings. Identity and authorization failures,
RequesterMustDiffer, stale decisions, already-decided waits, and legacy/manual
reconciliation states are rendered from sanitized service errors; the client
does not reconcile them itself.

Approval records the decision but never implies successful execution. Approved
runs require explicit Resume, retain the same durable run/wait correlation, and
continue through the existing M10 restore validation, M6 policy/audit, and M6.1
containment path. Deny and abort dispatch zero tools. Durable Waiting is rebuilt
from service state after desktop or service restart, and approval payloads are
never stored in browser storage.

Activity integrates Waiting, decision-ready, resuming, verifying, completed,
failed, and aborted states using safe service metadata. Server state remains
authoritative and the UI never optimistically reports execution success.

## M16.6 Durable Conversations and History

M16.6 adds passive, provider-neutral history reads without moving persistence
or recovery authority into the desktop:

```text
owner-authorized session page -> selected session run page
  -> safe status/event projections -> restored conversation UI
```

`SessionOwnershipPort` provides owner-filtered bounded session pages, while
`RunReadPort` provides bounded newest-first run pages for one session. The
SQLite adapter implements both behind the existing service boundary. The
service authorizes list and history operations from `VerifiedPrincipal` plus
durable ownership metadata; possession of a SessionId or RunId is insufficient.
No SQLite type, query, row identifier, or mutation handle crosses the port.

The HTTP adapter and Tauri bridge expose only `conversation_list_sessions` and
`conversation_list_runs`. Responses contain deterministic conversation titles,
last-activity metadata, latest disposition, workflow, event cursor, outcome,
result-availability flag, and bounded prior-run summaries. Strict Rust and Zod
decoders reject unknown fields, oversized pages, invalid IDs, and unsupported
workflow identifiers. No generic URL, method, SQL, or persistence command was
added to the WebView capability set.

TanStack Query loads session summaries first and run metadata only when a
conversation is selected. A restored run catches up through the existing
bounded event cursor and status commands; it never calls a start command or
replays model, knowledge, approval, tool, or containment work. Durable Waiting
continues through the M16.5 approval projection. Failed and manual
reconciliation runs remain visibly non-successful and are not client-repaired.

M13 application results remain intentionally volatile. A result still held by
the service is rendered authoritatively. If it was lost across service restart,
the desktop displays `Result unavailable` and does not regenerate an answer.
Conversation prompts, answers, citations, approval payloads, and run payloads
remain absent from browser storage; only the theme preference is local.
Previous run status metadata is available in the optional inspector. Rename,
archive, folders, tags, and search remain deferred.

## M16.7 Settings and Runtime Status

Settings is a lazy-loaded, observational surface. Local appearance preferences
support system, light, and dark themes. Safe runtime rows project only bounded
service, model, knowledge, workflow, LocalWrite, containment, identity,
version, budget, and authorized reconciliation metadata already exposed by the
service.

The surface cannot modify model or knowledge endpoints, MCP registration,
policy, identity roles, action-seal keys, containment, or deployment
configuration. It adds no generic configuration API. Operator-only operational
information remains gated by M15 service authorization rather than by desktop
navigation or local UI state.

## M16.8 Desktop Quality and Native Review

The quality pass aligns typography, spacing, borders, icon sizing, status
colors, Markdown, code, tables, citations, activity, HITL, and error states in
light and dark themes. The composer auto-grows within a bounded height, retains
Ctrl/Cmd+Enter submission, and keeps Send/Stop and offline semantics explicit.
Tail following yields to manual scroll and exposes `Jump to latest` without
forcing the viewport. Focus restoration, keyboard labels, reduced motion,
forced-color behavior, and zoom-safe layout are part of the shared foundation.

A real Tauri GTK/WebKit review covered `760x520`, approximately `1280x800`, and
`1700x900`, light and dark themes, and 100%, 150%, and 200% native GTK scaling.
Reviewed states included empty, active, completed Markdown with citations/code/
table, manual scroll, Waiting, approved-ready, degraded, unavailable,
cancelled, failed, unavailable result, reconciliation, Settings, Run Inspector,
and command palette.

The review found and fixed four UX defects:

- a redundant fixed WebView minimum height clipped content at 200% scaling;
- a failed health refetch could leave the shell visually connected;
- cancelled runs were styled and summarized as failures;
- manual reconciliation used generic failure wording instead of its explicit
  non-resumable status.

The Tauri native window minimum remains authoritative. No named command, ACL,
plugin, credential, transport, or execution capability changed in M16.8.

## Service States

The shell presents ready, degraded, unavailable, and draining states from the
existing bounded service metadata. Readiness and version details stay in the
inspector. Unavailable state disables the placeholder task path and offers only
a status refresh; it does not add retries or execution behavior to the UI.

## Verification

The completed M16.8 implementation passed:

- TypeScript typecheck and ESLint with zero warnings;
- 36 frontend tests covering conversation, history, HITL, settings, activity,
  cancellation, reconciliation, and unavailable-service behavior;
- Vite production build and Tauri release build;
- nine desktop Rust service-bridge tests and strict Clippy;
- capability, dependency-direction, and forbidden-surface scans;
- native visual and interaction review across compact, normal, wide, light,
  dark, and 100%/150%/200% scaling configurations.

The M16.5 closure review confirmed the exact `approval_submit_decision` command
and dedicated ACL. The pre-existing M11
`termination_reaps_process_group_descendant` regression reproduces in the
established container and remains outside M16.6; MCP lifecycle code was not
changed. Native host Clippy lacks the required GTK development libraries, so the
established Tauri build container supplied those system dependencies for strict
Clippy, Rust tests, and the final debug build.

## Guarantees and Limits

M16 provides a bounded local-service bridge, minimal WebView capability set,
responsive accessible design foundation, real ReadOnly conversation path, a
durable LocalWrite approval client, durable service-owned history, and
observational Settings over existing server authority.
Transport credentials are not exposed to JavaScript, clients cannot select the
workflow or infrastructure, and terminal results remain service-authoritative.
Activity and inspection are metadata projections only and do not create a new
event or result authority.

It does not provide desktop-side policy, approval authority, containment,
deployment mutation, ACP, pane resizing, packaging/signing, generic service
transport, or conversation payload persistence. Event subscription uses
bounded cursor polling instead of a persistent WebView SSE connection. Durable
history stores only existing service metadata; it does not make volatile M13
answers durable. Native screen-reader certification and automated pixel-diff
coverage remain future quality work.
Missing results are reported without replaying the model call.

## Forward Constraints

- Keep the WebView free of credentials and generic transport capabilities.
- Add only named typed commands for reviewed service operations.
- Preserve `agent-service` and `ExecutionHarness` as runtime authorities.
- Do not let clients select model, tool, MCP server, endpoint, policy, or
  containment infrastructure.
- Keep conversation payloads out of metadata-only operational surfaces.
- Preserve keyboard accessibility, reduced motion, and compact-window support.
- Keep terminal results authoritative over transient activity projections.
- Never infer unavailable evidence or usage metadata from citation content.
- Keep exact action bindings and approval authorization server-side; JavaScript
  may submit only public CAS decision fields.
- Never treat an approval decision as execution success; preserve explicit
  resume and service-authoritative terminal state.
- Keep history reads owner-authorized, bounded, metadata-only, and passive.
- Never replay external effects or infer a missing result during restoration.
- Keep Settings observational and local appearance preferences non-authoritative.
- Do not widen WebView capabilities for visual, accessibility, or UX changes.
