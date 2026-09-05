---
title: "M8 Enterprise Knowledge Integration"
tags: ["m8", "enterprise-knowledge", "rag", "mcp", "retrieval"]
created: 2026-09-05
updated: 2026-09-05
sources: ["crates/agent-knowledge/src/lib.rs", "crates/agent-knowledge-adapters/src/", "crates/agent-harness/src/execution.rs", "crates/agent-harness/src/recovery.rs", "crates/agent-loop/src/program.rs"]
links: ["enterprise-local-agent-milestone-index.md", "m7-durable-event-persistence-and-recovery.md"]
category: architecture
confidence: high
schemaVersion: 1
---

# M8 Enterprise Knowledge Integration

## Status

Implemented and verified in the working tree. No M8 commit or tag has been
created.

## Scope

M8 integrates exactly two existing read-only knowledge systems: the OCPP
RAG/KAG backend and the standards-mcp ISO/standards backend. Codebase Memory is
not an M8 enterprise backend. M8 adds no database, embedding, indexing,
ingestion, chunking, vector store, web search, graph engine, or autonomous
retrieval loop.

## Architecture

```text
agent-knowledge-adapters -> agent-knowledge
agent-harness -> agent-knowledge -> agent-core
agent-loop -> agent-harness + agent-knowledge
```

`agent-knowledge` owns `KnowledgePort`, bounded requests and evidence,
single/federated routing, deterministic merge and deduplication, typed safe
metadata, provenance, isolated native-score systems, and grounded model request
construction. `agent-knowledge-adapters` owns transport and backend response
types. MCP and HTTP types do not enter core, harness public domain events, or
loop control.

## Backend Contracts

The OCPP MCP tools were inspected first. `get_ocpp_evidence_pack` and
`search_ocpp_knowledge` return Markdown-only content, and the server converts
backend exceptions into ordinary `Error: ...` text. M8 therefore uses the
structured read-only HTTP `GET /search` endpoint with fixed query parameters:
`q`, `top_k`, `max_chars`, `include_content=true`, and `include_query=false`.
Its `SearchResponse` contains correlation metadata and ordered
`ScoredChunkResponse` records.

The standards adapter uses only MCP `search_standards_kag`. It sends the exact
bounded arguments `query`, `limit`, `include_preview=true`, `provider=local`,
`model=BAAI/bge-m3`, `pool_limit=20`, `graph_weight=0.001`, and
`review_status=null`. The structured result includes source/chunk provenance,
section/heading/page range, chunk type, backend-specific KAG scores,
relationship counts, related entities, and a bounded content preview. Only the
fields needed by provider-neutral evidence are retained.

## Bounds and Security

Queries are at most 1 KiB, results at most 8, transport responses at most 256
KiB, each evidence content item at most 4 KiB, and total evidence content at
most 16 KiB. No generic token estimate is claimed. Payload-bearing request and
evidence types redact content from `Debug`.

Trusted configuration selects `Single` or ordered `Federated` routing with
`RequireAll` or `AllowPartial`. Merge order is backend rank followed by trusted
backend priority and stable evidence ID. Native scores are never compared
between backends. Deduplication uses a canonical reference when available and a
bounded content digest otherwise. Partial results are explicitly degraded.

Retrieved content is marked as untrusted data in grounded model requests. This
does not claim prompt-injection immunity. Retrieval cannot invoke or acquire
tool, policy, approval, or containment authority.

## Persistence and Recovery

Event schema version 7 adds metadata-only knowledge start, restart, completion,
failure, and model-grounding events. Checkpoint schema version 2 records only
replay-critical knowledge metadata. Query and evidence content are absent from
events and checkpoints.

Replay never calls `KnowledgePort`. An interrupted retrieval is not treated as
an indeterminate side effect. Only `RestartableRetrieval` programs with a
matching version may reconstruct the exact query and route and issue a new
read. The same snapshot is required when a backend supports one. Neither
current backend exposes a trustworthy corpus snapshot in its canonical search
response, so resumed retrieval is explicitly fresh, not historical replay.

## Guarantees and Limits

M8 guarantees fixed backend operations, deterministic bounded normalization,
metadata-only recovery records, inert replay, and unchanged M5-M7 execution
authority. It does not guarantee backend availability, comparable cross-backend
scores, snapshot stability where a backend supplies no snapshot, prompt-
injection immunity, or cancellation of work internal to a remote HTTP server.
The stdio MCP process is directly terminated on dropped invocation; descendant
process-tree containment is not claimed for knowledge adapters.
