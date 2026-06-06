---
# kinora-hgpl
title: 'Render/sync engine: kinograph-bound, idempotent, region-preserving'
status: todo
type: feature
priority: high
created_at: 2026-06-06T09:28:31Z
updated_at: 2026-06-06T09:28:43Z
parent: kinora-bm7z
blocked_by:
    - kinora-q28s
    - kinora-thow
---

Given a file's stencil:kinograph binding, load the api-kinograph (via kinora Kinograph), and for each slot match the entry by name, resolve it (pin or head, via kinora Resolver + a kind-scoped name lookup), split the spec kino, and write the read-only block (doc-comments from prose + code). Preserve editable regions byte-for-byte. Idempotent re-run (no-op when ro hash unchanged). Warn on hand-edited read-only regions (hash mismatch). Error on slot with no matching entry; collect entries with no slot. TDD.
