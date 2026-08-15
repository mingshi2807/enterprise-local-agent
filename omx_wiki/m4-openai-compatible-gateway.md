---
title: "M4 OpenAI Compatible Gateway"
tags: ["m4", "openai-compatible", "gateway", "qwen", "rig", "rustls"]
created: 2026-08-15T13:34:12.966Z
updated: 2026-08-15T13:34:12.966Z
sources: ["docs/proposal/proposal.md:3348-4302", "docs/reports/reports_impl.md:563-751", "git tag m4-openai-compatible-gateway"]
links: ["enterprise-local-agent-milestone-index.md", "m3-rig-model-adapter.md", "m5-typed-action-planning.md"]
category: architecture
confidence: high
schemaVersion: 1
---

# M4 OpenAI-Compatible Gateway

## Status

Completed and committed as `1050d77` / tag `m4-openai-compatible-gateway`.

## Objective

Add production-capable OpenAI-compatible model composition for local Qwen or a future enterprise gateway while preserving the M0–M3 boundaries.

## Approved architecture and review decisions

- Keep rig-core 0.41.0 and enable exactly the rustls feature.
- agent-provider-rig owns OpenAI-compatible client/model construction; the CLI owns environment loading and live/fake selection.
- BearerCredential is mandatory, validates nonempty content, has redacted Debug, and is not cloneable, displayable, or serializable.
- Base URLs use url::Url, reject credentials/query/fragment and chat-completions endpoint paths, and normalize only one trailing slash.
- HTTP is permitted only for localhost or loopback IPs; all other hosts require HTTPS.
- Model identifiers are opaque configuration, not Rust enums.
- Factory output is Arc<dyn ModelPort>; no additional gateway abstraction or ModelPort wrapper is introduced.
- No timeout, retry, cancellation token, readiness call, fallback, streaming, or tool definition is added.
- A bounded private HTTP/1.1 test server verifies deterministic wire behavior without external network access.
- Live access is explicit through `--live-openai-compatible`; default execution remains network-free.

## Implemented result

Added OpenAiCompatibleConfig, ProviderLabel, BearerCredential, validated provider construction, bounded HTTP integration infrastructure, and explicit live CLI composition.

## Verification evidence

Historical verification: 105 unit/integration tests plus two compile-fail doctests passed. Five HTTP integration tests, strict Clippy, formatting, dependency/boundary scans, and default 1/1/1 CLI smoke passed. External live mode was not run.

## Deliberate deferrals

Readiness, retries, proxy/custom CA/mTLS, custom headers, routing/fallback, no-auth mode, native tool calling, structured output, streaming, OVH SDK, and external live CI.

## Historical sources

- Proposal and approval: `docs/proposal/proposal.md`, lines 3348–4302.
- Implementation report: `docs/reports/reports_impl.md`, lines 563–751.
- Index: [[enterprise-local-agent-milestone-index]].
- Previous: [[m3-rig-model-adapter]]. Next: [[m5-typed-action-planning]].
