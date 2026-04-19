---
# kinora-mjvb
title: 'Hot ledger: one-file-per-event (.kinora/hot/)'
status: in-progress
type: feature
priority: normal
created_at: 2026-04-19T05:46:36Z
updated_at: 2026-04-19T05:52:33Z
parent: kinora-xi21
---

First phase of `kinora-xi21`. Swaps the ledger's write unit from "append to a per-lineage file" to "create a new file per event." The resulting `.kinora/hot/` directory is structurally immutable (every file written once), sharded like the blob store, and trivially merge-safe across git branches (set-union of files, zero JSONL conflicts).

This phase is independent of the root-kinograph model — it stands on its own as a merge-safety improvement — and is a prerequisite for every later phase (`root` kind, compaction, multiple roots).

## Design

### File layout

```
.kinora/
  hot/
    <ab>/
      <event-hash>.jsonl    # single line; immutable after write
```

- `<event-hash>` = BLAKE3 of the canonical JSON encoding of the event
- Sharded by first two hex chars of the event hash (matches blob store convention)
- Each file contains exactly one JSONL line, terminated by `
`
- Files are written once and never modified

### Event schema

Unchanged from today in terms of fields (`kind`, `id`, `hash`, `parents`, `ts`, `author`, `provenance`, `metadata`). The event hash is a new derived property — computed at write time from a canonical serialization and used as the filename.

Canonicalization: sort metadata keys, strip trailing whitespace, use fixed JSON encoding. Must be deterministic so that the same logical event always produces the same event hash (dedup across branches that independently record the same event).

### Readers

- `Ledger::read_all_events()` globs `hot/*/*.jsonl`, reads each, dedups by event hash
- In-memory index (built on load) groups events by `id` to reconstruct per-kino history
- The per-kino "read lineage" operation becomes: filter in-memory index by id, sort by `(ts, hash)`

### HEAD semantics

Current `HEAD` points at a lineage shorthash. Under this model, HEAD is less meaningful — kinos are discovered by scanning hot events. For compatibility during transition, keep HEAD but make it advisory (points at the most recently mutated lineage shorthash for the `current_lineage()` helper used by render).

### Migration

Existing `.kinora/ledger/<shorthash>.jsonl` files are read at load time and their events folded into the in-memory index (so existing data keeps working). A separate `kinora migrate` command (or the first compaction when the `root` phase lands) converts them to `hot/*/*.jsonl`. Phase 1 does **not** force migration — it just teaches the reader to handle both.

## Acceptance

- [ ] New `.kinora/hot/<ab>/<event-hash>.jsonl` layout implemented in `kinora::ledger`
- [ ] Canonical event encoding + event-hash computation
- [ ] `store` writes to `hot/` (not `ledger/`)
- [ ] Readers transparently union events from both `ledger/` (legacy) and `hot/` (new) for migration
- [ ] Dedup by event hash is idempotent (same event stored twice = one file)
- [ ] Tests: write+read roundtrip, dedup, cross-branch merge simulation, legacy ledger coexistence
- [ ] Zero compiler warnings
- [ ] Existing CLI tests (`store`, `resolve`, `render`) pass unchanged

## Non-goals (deferred to later phases)

- `root` kind introduction
- Compaction on main
- `assign` event type
- GC/prune of old events
- Merkle sub-kinographs

## Open questions

- Exact canonical encoding: JSON with sorted keys? styx? The choice affects how robust event-hash determinism is across implementations.
- When both `ledger/` (legacy) and `hot/` (new) are present, which takes precedence for a given event that exists in both? (Dedup handles it; order doesn't matter semantically.)
- Should `HEAD` go away entirely in this phase, or stay advisory until the `root` phase?


## Plan

TDD three-commit split:

1. **Tests commit**: add `paths::hot_*` + `Event::event_hash` + `Ledger::write_event`/`read_all_events` stubs (unimplemented); add failing tests that exercise the new API (write roundtrip, dedup, legacy coexistence, shard layout, path shape).
2. **Implementation commit**: fill in bodies; switch `store_kino` to `write_event`; rewrite `Resolver::load` on top of `read_all_events`; deprecate `ledger/` write path; adapt/remove the branch-aware-tiebreak test (HEAD-based disambiguation is superseded by the root-phase; phase 1 lets forks always emit `MultipleHeads`).
3. **Review commit** (if needed).

## Design choices

- **Canonical encoding for event-hash**: reuse `Event::to_json_line()` (facet-json over `BTreeMap` → sorted keys, `Vec` preserves order, struct fields in declaration order → deterministic). Matches precedent set by current `mint_and_append` which already hashes the JSON line.
- **Event hash** is full 64-hex BLAKE3 of the canonical line; filename `hot/<ab>/<full-hash>.jsonl` so one shard dir stays searchable and collides with `store/<ab>/` naming.
- **Dedup**: write uses `OpenOptions::create_new` — if file exists, no-op (content-addressed, so content is guaranteed identical). Readers union legacy + hot, dedup by event-hash.
- **Legacy lineages**: keep `ledger_dir`/`read_all_lineages` *read path* working; stop writing there. `Identity.lineages` field keeps the shape; new entries get event-hash-shorthash as their "lineage" label.
- **HEAD**: no longer written by `store_kino`. Still read for `current_branch_label` and `head_for_current_lineage` (legacy only). In absence, branch label defaults to `"main"`.
- **StoredKino**: keep field names (`lineage`, `was_new_lineage`) for back-compat; `lineage` = event-hash-shorthash, `was_new_lineage` = true iff file didn't already exist.
