---
title: "M16 Desktop Foundation, Conversation, Run Inspector, and HITL"
tags: ["m16", "desktop", "tauri", "react", "design-system", "app-shell", "conversation", "readonly", "activity", "inspector", "hitl", "localwrite", "approval"]
created: 2026-09-21
updated: 2026-09-22
sources: ["apps/agent-desktop/src-tauri/src/service_client.rs", "apps/agent-desktop/src-tauri/src/lib.rs", "apps/agent-desktop/src-tauri/capabilities/main.json", "apps/agent-desktop/src/app/App.tsx", "apps/agent-desktop/src/app/CommandPalette.tsx", "apps/agent-desktop/src/bridge/contracts.ts", "apps/agent-desktop/src/bridge/service.ts", "apps/agent-desktop/src/queries/conversation.ts", "apps/agent-desktop/src/features/runActivity.ts", "apps/agent-desktop/src/components/ApprovalPanel.tsx", "apps/agent-desktop/src/components/ConversationWorkspace.tsx", "apps/agent-desktop/src/components/MarkdownAnswer.tsx", "apps/agent-desktop/src/components/RunInspector.tsx", "apps/agent-desktop/src/components/SessionSidebar.tsx", "apps/agent-desktop/src/styles/tokens.css"]
links: ["enterprise-local-agent-milestone-index.md", "m15-enterprise-identity-authorization.md", "m12-agent-service-api.md", "m14-deployment-operations-hardening.md"]
category: architecture
confidence: high
schemaVersion: 1
---

# M16 Desktop Foundation, Conversation, Run Inspector, and HITL

## Status

M16.0 architecture was approved. M16.1 and M16.2 are implemented and committed
as `29942c3` and `0672edf`. M16.3 through M16.5 are implemented and verified in
the current worktree. No M16 milestone tag has been created.

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

## Service States

The shell presents ready, degraded, unavailable, and draining states from the
existing bounded service metadata. Readiness and version details stay in the
inspector. Unavailable state disables the placeholder task path and offers only
a status refresh; it does not add retries or execution behavior to the UI.

## Verification

The current M16.5 implementation passed:

- TypeScript typecheck and ESLint with zero warnings;
- twenty-three frontend tests, including five focused approval tests covering
  durable Waiting discovery, trusted preview, Approve, Deny, explicit Resume,
  abort, stale and unauthorized failures, and payload rejection;
- Vite production builds and Tauri debug build with `--no-bundle`;
- strict workspace Rust Clippy and eight Rust service-bridge tests;
- capability, dependency-direction, and forbidden-surface scans;
- `cargo fmt --all -- --check` and a full workspace test pass.

The M16.5 closure review confirmed the exact `approval_submit_decision` command
and dedicated ACL. An intermittent pre-existing M11 process-reaping test was
not changed: current and pre-M16.5 MCP sources were byte-identical, isolated and
repeated comparison runs passed on both revisions, and the final workspace run
passed. Native host Clippy lacks the required GTK development libraries, so the
established Tauri build container supplied those system dependencies for strict
Clippy, Rust tests, and the final debug build.

## Guarantees and Limits

M16 provides a bounded local-service bridge, minimal WebView capability set,
responsive accessible design foundation, real ReadOnly conversation path, and
a durable LocalWrite approval client over existing server authority.
Transport credentials are not exposed to JavaScript, clients cannot select the
workflow or infrastructure, and terminal results remain service-authoritative.
Activity and inspection are metadata projections only and do not create a new
event or result authority.

It does not provide desktop-side policy, approval authority, containment,
settings, ACP, pane resizing, packaging/signing, generic service transport, or
conversation payload persistence. Event subscription uses bounded cursor
polling instead of a persistent WebView SSE connection. The service has no
standalone session-list/read route, so current desktop conversation navigation
is process-local. Volatile M13 results may be unavailable after service restart
and are reported without replaying the model call.

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
