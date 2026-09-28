# M19 Production Release Record

- Product: Enterprise Local Agent
- Version: 1.0.0
- Tag: `v1.0.0`
- Release date: 2026-09-28
- Supported platform: Linux x86_64
- Compatibility fingerprint: `bc71818690ec583a7dc36b0d6c9ae21954804b891ea142a4157f28da33a683a3`
- Service deployment: separate
- Automatic updates: disabled

The final commit and artifact hashes are authoritative in the generated release
manifest under `release/v1.0.0/`. The manifest must record
`source_dirty=false` and the commit referenced by `v1.0.0`.

## Dependency Review

The M18 Project Owner / Release Authority approval dated 2026-09-27 covered
the exact RC dependency graph. M19 changes only first-party package versions
from `0.1.0-rc.2` to `1.0.0`; the resolved third-party Cargo and npm package
sets are unchanged. `THIRD-PARTY-LICENSES.json` records the final locked graph,
lockfile hashes, package versions, sources, and declared licenses.

## Architecture Freeze

M19 introduces no feature and no authority change. The desktop remains a thin
client over the separately deployed service. M5 validation, M6 policy/approval/
audit, M6.1 containment, M7 recovery, M10 durable HITL, M15 authorization, and
M17 compatibility hardening remain authoritative.
