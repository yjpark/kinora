---
# kinora-wpup
title: 'Store filename: append kind-derived extension (<hash>.<ext>)'
status: todo
type: task
priority: normal
created_at: 2026-04-19T04:21:36Z
updated_at: 2026-04-19T04:21:36Z
parent: kinora-w7w0
---

## Design

Extension derived from kind at write time:

| Kind         | Ext      |
|--------------|----------|
| markdown     | `.md`    |
| text         | `.txt`   |
| kinograph    | `.styx`  |
| binary       | (none, or from metadata hint later) |
| `prefix::x`  | `.bin`   |

### Reads

Readers index by 64-hex hash; the extension is just decoration. Two options:
- **Glob `<hash>.*`** — simple, honest about the fact that dedup is hash-only so a blob's extension may not match every ledger event's kind
- **Recompute ext from ledger event kind** — strict; one extra lookup per read

Prefer glob — matches the "extension is a UX hint, not a key" framing.

### Dedup semantics unchanged

Same content under different kinds still produces one blob (first writer wins on extension). The ledger event records the kind for correctness; on-disk extension is advisory.

## Acceptance

- [ ] `write_content(kind, bytes)` writes to `<hash[..2]>/<hash>.<ext>`
- [ ] `read_content(hash)` resolves via glob (or equivalent); returns bytes regardless of extension
- [ ] Existing `.kinora/store/` layouts (extensionless) still readable, or migration noted
- [ ] Tests cover: roundtrip per kind, glob read, same-content-different-kind dedup, namespaced-kind fallback
- [ ] RFC-0003 re-import (or migration) produces `.md` on disk

## Notes

Discovered while dogfooding RFC-0003 (kinora-cium) — the extensionless blob is annoying to open directly. Cheap change, clear upside.
