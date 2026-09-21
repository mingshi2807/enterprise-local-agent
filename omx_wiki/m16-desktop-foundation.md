---
title: "M16 Desktop Foundation, App Shell, and ReadOnly Conversation"
tags: ["m16", "desktop", "tauri", "react", "design-system", "app-shell", "conversation", "readonly"]
created: 2026-09-21
updated: 2026-09-21
sources: ["apps/agent-desktop/src-tauri/src/service_client.rs", "apps/agent-desktop/src-tauri/src/lib.rs", "apps/agent-desktop/src-tauri/capabilities/main.json", "apps/agent-desktop/src/app/App.tsx", "apps/agent-desktop/src/app/CommandPalette.tsx", "apps/agent-desktop/src/bridge/contracts.ts", "apps/agent-desktop/src/bridge/service.ts", "apps/agent-desktop/src/queries/conversation.ts", "apps/agent-desktop/src/components/ConversationWorkspace.tsx", "apps/agent-desktop/src/components/MarkdownAnswer.tsx", "apps/agent-desktop/src/components/SessionSidebar.tsx", "apps/agent-desktop/src/components/ReadinessInspector.tsx", "apps/agent-desktop/src/styles/tokens.css"]
links: ["enterprise-local-agent-milestone-index.md", "m15-enterprise-identity-authorization.md", "m12-agent-service-api.md", "m14-deployment-operations-hardening.md"]
category: architecture
confidence: high
schemaVersion: 1
---

# M16 Desktop Foundation, App Shell, and ReadOnly Conversation

## Status

M16.0 architecture was approved. M16.1 and M16.2 are implemented and committed
as `29942c3` and `0672edf`. M16.3 is implemented and verified in the current
worktree. No M16 milestone tag has been created.

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
`version` commands. M16.3 adds five reviewed conversation commands without
adding generic transport. There are no filesystem, shell, generic HTTP, SQL,
model, MCP, persistence, or containment plugins. CSP disables WebView network
connections.

## M16.2 Conversation-First Shell

The desktop now provides:

- a compact application bar and subtle connection footer;
- a collapsible session sidebar with clearly labelled mock layout data;
- a dominant central conversation/task workspace;
- a read-only task composer that cannot dispatch work;
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

## Service States

The shell presents ready, degraded, unavailable, and draining states from the
existing bounded service metadata. Readiness and version details stay in the
inspector. Unavailable state disables the placeholder task path and offers only
a status refresh; it does not add retries or execution behavior to the UI.

## Verification

The current M16.3 implementation passed:

- TypeScript typecheck and ESLint with zero warnings;
- twelve frontend tests covering fixed workflow start, keyboard submission,
  one active run, cancellation, final answers, citations, model/knowledge and
  malformed-result failures, cursor catch-up, volatile result loss, payload
  rejection, shell states, and compact layout;
- Vite production builds and Tauri debug build with `--no-bundle`;
- strict Rust Clippy and six Rust service-bridge tests;
- capability, dependency-direction, and forbidden-surface scans;
- `cargo fmt --all -- --check` and clean diff validation.

M16.3 regenerates Tauri ACL files for the five named conversation commands. A
non-failing Vite advisory reports a roughly 697 KiB initial JavaScript chunk;
code splitting remains deferred until a real route boundary exists.

## Guarantees and Limits

M16 provides a bounded local-service bridge, minimal WebView capability set,
responsive accessible design foundation, and real ReadOnly conversation path.
Transport credentials are not exposed to JavaScript, clients cannot select the
workflow or infrastructure, and terminal results remain service-authoritative.

It does not provide LocalWrite, approval, durable Waiting interaction,
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
