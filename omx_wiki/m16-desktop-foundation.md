---
title: "M16 Desktop Foundation and App Shell"
tags: ["m16", "desktop", "tauri", "react", "design-system", "app-shell"]
created: 2026-09-21
updated: 2026-09-21
sources: ["apps/agent-desktop/src-tauri/src/service_client.rs", "apps/agent-desktop/src-tauri/capabilities/main.json", "apps/agent-desktop/src/app/App.tsx", "apps/agent-desktop/src/app/CommandPalette.tsx", "apps/agent-desktop/src/components/ConversationWorkspace.tsx", "apps/agent-desktop/src/components/SessionSidebar.tsx", "apps/agent-desktop/src/components/ReadinessInspector.tsx", "apps/agent-desktop/src/styles/tokens.css"]
links: ["enterprise-local-agent-milestone-index.md", "m15-enterprise-identity-authorization.md", "m12-agent-service-api.md", "m14-deployment-operations-hardening.md"]
category: architecture
confidence: high
schemaVersion: 1
---

# M16 Desktop Foundation and App Shell

## Status

M16.0 architecture was approved. M16.1 and M16.2 are implemented and committed
as `29942c3` and `0672edf`. No M16 milestone tag has been created.

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
execution authority moves into the desktop. Future conversation operations
must remain typed service requests and cannot call runtime ports directly.

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

The WebView capability contains only named `health`, `readiness`, and `version`
commands. There are no filesystem, shell, generic HTTP, SQL, model, MCP,
persistence, or containment plugins. CSP disables WebView network connections.

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

## Service States

The shell presents ready, degraded, unavailable, and draining states from the
existing bounded service metadata. Readiness and version details stay in the
inspector. Unavailable state disables the placeholder task path and offers only
a status refresh; it does not add retries or execution behavior to the UI.

## Verification

The completed foundation passed:

- TypeScript typecheck and ESLint with zero warnings;
- eight frontend tests covering service states, draining precedence, command
  palette behavior, pane shortcuts, editing safeguards, and compact layout;
- Vite production builds and Tauri debug build with `--no-bundle`;
- strict Rust Clippy and four Rust service-bridge tests;
- capability, dependency-direction, and forbidden-surface scans;
- manual Chromium review at compact light, normal light, and wide dark sizes.

The Tauri permission files were unchanged by M16.2. A non-failing Vite advisory
reports a roughly 529 KiB initial JavaScript chunk; code splitting is deferred
until a real route or conversation boundary exists.

## Guarantees and Limits

M16 provides a bounded local-service bridge, minimal WebView capability set,
responsive accessible design foundation, and polished non-executing desktop
shell. Transport credentials are not exposed to JavaScript.

It does not yet provide conversation execution, workflow selection, event
streaming, approval, durable Waiting interaction, run cancellation, settings,
ACP, pane resizing, packaging/signing, or a generic service API. Browser-only
visual review cannot exercise Tauri commands; service-state behavior is covered
by deterministic frontend and Rust bridge tests.

## Forward Constraints

- Keep the WebView free of credentials and generic transport capabilities.
- Add only named typed commands for reviewed service operations.
- Preserve `agent-service` and `ExecutionHarness` as runtime authorities.
- Do not let clients select model, tool, MCP server, endpoint, policy, or
  containment infrastructure.
- Keep conversation payloads out of metadata-only operational surfaces.
- Preserve keyboard accessibility, reduced motion, and compact-window support.
