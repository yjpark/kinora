---
# kinora-5k13
title: 'Data layer: content store + append-only ledger'
status: in-progress
type: feature
priority: normal
created_at: 2026-04-18T09:16:59Z
updated_at: 2026-04-18T15:34:48Z
parent: kinora-w7w0
blocked_by:
    - kinora-fhw1
---

Library-level content store and ledger that back the CLI commands. Follows decisions in `kinora-fhw1`.

RFC-0003 sections: *Repository Layout*, *Content Store*, *Ledger*, *Provenance*.

## Design

### Repository layout

```
.kinora/
  config.styx                  # required: repo-url; optional: author, render options
  HEAD                         # pointer to current lineage file
  store/
    aa/
      aabbccdd…                # BLAKE3-addressed blob, sharded by first 2 hex chars
  ledger/
    <shorthash>.jsonl          # per-lineage append-only JSONL
    <shorthash>.jsonl          # filename = first 8 hex of BLAKE3 of first event
```

### Content store

- BLAKE3 plain hash of content → 64-char lowercase hex
- Sharded path: `store/<first-2-hex>/<full-hex>`
- Pure content; no system-injected metadata inside blobs
- Read verifies hash matches path

### Ledger event envelope (JSONL)

```json
{
  "kind": "<content-type>",
  "id": "<identity-hash>",
  "hash": "<content-hash>",
  "parents": ["<hash>", ...],
  "ts": "2026-04-18T09:20:00Z",
  "author": "yj",
  "provenance": "…",
  "metadata": { ... }
}
```

- `kind`: content type (`markdown`, `text`, `binary`, `kinograph`, or `prefix::extension`)
- `id`: identity = BLAKE3 of first content version
- `hash`: this version's content hash (= `id` for birth event)
- `parents`: content hashes that influenced this version (linear: 1; fork: 0 of same identity; merge: 2+; detach/combine: cross-identity)
- Metadata keys follow namespace rules

### Lineage management

- `.kinora/HEAD` stores current lineage filename
- First `store` on a new git branch mints a new lineage (content-addressed filename from first event)
- Subsequent stores append to current lineage
- Branch detection via `git rev-parse` + commit relationships

### Metadata merge

- Per-field, ts-latest wins
- Events carry only changed fields
- `null` removes a field
- Wholesale array replacement (no CRDT in MVP)

### Validation

- Bare metadata keys must be in Kinora's known set; unknown = reject
- Namespaced keys (`prefix::…`) preserved as-is
- Parent hashes must exist in store
- `id` consistency: birth event has `id == hash` and `parents == []`; version events must refer to an existing identity's history

## Acceptance

- [x] `.kinora/config.styx` parsed; `repo-url` required
- [x] Content store writes and reads BLAKE3-addressed blobs with sharded layout
- [x] Content store round-trips preserve exact bytes
- [x] Ledger appends JSONL events to current lineage file
- [x] Ledger never modifies or deletes prior entries
- [x] Event envelope enforced: `kind`, `id`, `hash`, `parents[]`, `ts`, `author`, `provenance`, `metadata{}`
- [x] Namespace rules validated on write (bare reserved, `prefix::` extension)
- [x] Parent existence checked on append
- [ ] First-store-on-new-branch mints new lineage file
- [x] `.kinora/HEAD` tracks current lineage
- [x] facet-based serialization for in-memory types
- [x] Unit tests cover round-trip, lineage creation, append invariants, metadata merge


## `kinora init` (folded in)

Bootstraps `.kinora/` in a repo:

1. Refuse if `.kinora/` already exists
2. Resolve `repo-url`:
   - `--repo-url URL` if given
   - else `git remote get-url origin` (via `gix`)
   - else error, prompt user to pass `--repo-url`
3. Create `.kinora/` with:
   - `config.styx` containing `repo-url`
   - empty `store/` and `ledger/` directories
   - no `HEAD` yet (minted on first `store`)

### Acceptance (init)

- [ ] `kinora init` creates `.kinora/` with `config.styx` (only `repo-url`) + empty `store/` + empty `ledger/`
- [ ] `--repo-url URL` overrides git remote
- [ ] Falls back to `git remote get-url origin` via `gix`
- [ ] Errors clearly if no remote and no flag given
- [ ] Refuses to overwrite an existing `.kinora/`


## Plan

Module layout under `crates/kinora/src/`:

- `hash.rs` — `Hash` newtype (64-char lowercase hex), BLAKE3 helpers, hex parse/display
- `paths.rs` — path builders for store (`store/aa/aabb…`), ledger (`ledger/<shorthash>.jsonl`), HEAD, config
- `value.rs` — `Value` enum + `Metadata` alias (nested JSON-like values for metadata fields)
- `config.rs` — `Config` struct, `.kinora/config.styx` parse (only `repo-url` in MVP)
- `namespace.rs` — bare vs `prefix::name` validation (reserved bare set)
- `store.rs` — `ContentStore` (write + read + dedupe + verify-on-read)
- `ledger.rs` — `Event` envelope + `Ledger` (append + scan, lineage file resolution)
- `init.rs` — `kinora init` flow (refuse-if-exists, repo-url resolution, layout creation)

Commit plan (TDD per module):

1. `hash` + `paths` (foundational primitives)
2. `value` + `namespace` (metadata substrate + validation)
3. `config` (depends on value)
4. `store` (depends on hash + paths)
5. `ledger` (depends on all of above)
6. `init` (depends on config + paths)
7. Subagent review + any fixes

Each commit: tests first, then impl, then bean checkbox updates. Zero warnings at every step (check `.bacon-claude-diagnostics` before committing).
