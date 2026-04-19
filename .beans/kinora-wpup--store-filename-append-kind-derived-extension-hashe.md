---
# kinora-wpup
title: 'Store filename: append kind-derived extension (<hash>.<ext>)'
status: completed
type: task
priority: normal
created_at: 2026-04-19T04:21:36Z
updated_at: 2026-04-19T06:23:03Z
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

- [x] `write_content(kind, bytes)` writes to `<hash[..2]>/<hash>.<ext>`
- [x] `read_content(hash)` resolves via glob (or equivalent); returns bytes regardless of extension
- [x] Existing `.kinora/store/` layouts (extensionless) still readable, or migration noted
- [x] Tests cover: roundtrip per kind, glob read, same-content-different-kind dedup, namespaced-kind fallback
- [~] RFC-0003 re-import (or migration) produces `.md` on disk — explicitly deferred; legacy blobs remain readable via `find_blob_path`, so no automatic migration is needed. Re-storing would duplicate ledger events with new timestamps.

## Notes

Discovered while dogfooding RFC-0003 (kinora-cium) — the extensionless blob is annoying to open directly. Cheap change, clear upside.

## Summary of Changes

Landed in three commits:

1. **`store: add ext_for_kind + find_blob_path helpers (kinora-wpup)`** (f11a5ed) — `ext_for_kind` in `namespace.rs` maps reserved kinds → extensions (`markdown`→`md`, `text`→`txt`, `kinograph`→`styx`, `binary`→none, namespaced→`bin`). `find_blob_path` in `paths.rs` scans a shard dir and matches by hash-stem, so readers don't need to know the extension.
2. **`store: filename has kind-derived extension (kinora-wpup)`** (1257e05) — `ContentStore::write(kind, bytes)` now picks the extension; `read`/`exists` use `find_blob_path`. Legacy extensionless blobs still resolve. Dedup unchanged: first writer wins the extension; same content under a different kind returns the existing hash without rewriting.
3. **`store: tmp filename no longer matches hash-stem scan (kinora-wpup)`** (614de46) — review-fix. Tmp name changed from `<hash>.tmp` to `.tmp-<hash>[.ext]` so a crashed partial write can't be mistaken for a real blob by the stem scan. Test pins the behavior.

### Known limitations

- A rare multi-writer race on the **same content, different kinds** could produce two blobs differing only in extension if both writers pass the `find_blob_path` check before either renames into place. Semantically harmless — the hash still uniquely identifies the content, and all readers find one via stem match. Not worth fencing with a lock for the single-writer agent workflow we have today. Revisit if multi-agent concurrent writes become real.
- Migration of legacy extensionless blobs is explicitly not done. `find_blob_path` handles both layouts, so there's no read-path pressure. Re-storing existing kinos would duplicate ledger events under new timestamps, so automatic migration is the wrong move.
