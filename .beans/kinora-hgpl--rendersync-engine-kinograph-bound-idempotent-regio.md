---
# kinora-hgpl
title: 'Render/sync engine: kinograph-bound, idempotent, region-preserving'
status: completed
type: feature
priority: high
created_at: 2026-06-06T09:28:31Z
updated_at: 2026-06-10T00:40:20Z
parent: kinora-bm7z
blocked_by:
    - kinora-q28s
    - kinora-thow
---

Given a file's stencil:kinograph binding, load the api-kinograph (via kinora Kinograph), and for each slot match the entry by name, resolve it (pin or head, via kinora Resolver + a kind-scoped name lookup), split the spec kino, and write the read-only block (doc-comments from prose + code). Preserve editable regions byte-for-byte. Idempotent re-run (no-op when ro hash unchanged). Warn on hand-edited read-only regions (hash mismatch). Error on slot with no matching entry; collect entries with no slot. TDD.

## Todo

- [x] `engine::sync_file(file, resolver, target)` — kinograph-bound resolution
- [x] Match slots to entries by name; resolve (pin or head); split spec; render ro block (doc + code)
- [x] Preserve editable regions; apply slot indent to ro content
- [x] Status classification: Created / Updated / Unchanged / DriftOverwritten / Unmatched
- [x] Reports: unslotted entries, orphan ro blocks; `changed()` / `drifted()`
- [x] Errors: NoBinding, NotApiKinograph, NotApiSpec, DuplicateEntryName; wire into `StencilError`
- [x] Tests + zero warnings + code review (lazy resolution + duplicate-name hardening)

## Summary of Changes

Added `engine::sync_file` — the render/sync core that ties the region model + spec model to kinora.

- Reads the file's `stencil:kinograph` binding, resolves the api-kinograph (kind-checked), and indexes its entries by name (`build_index`).
- For each agent-placed slot: matches the entry by name, resolves it (`pin` → `resolve_at_version`, else head `resolve_by_id`), splits the spec kino (`SpecItem`), and writes a read-only block = `///` doc-comment (from prose) above the signature code, indented to the slot.
- Preserves all editable/Text/Binding blocks byte-for-byte; only the slot-owned ro block is (re)written. Slot inherits indentation.
- Idempotent: the ro marker carries the source content hash, so an unchanged source renders identically → `Unchanged`. Status: `Created` / `Updated` (source moved) / `Unchanged` / `DriftOverwritten` (hand-edited ro region, same hash) / `Unmatched`.
- Reports: per-slot outcomes, `unslotted_entries`, orphan ro blocks; helpers `changed()` / `drifted()` / `unmatched()`.
- Errors (wired into `StencilError`): `NoBinding`, `NotApiKinograph`, `NotApiSpec`, `DuplicateEntryName`.

**Review hardening:** entry resolution is tolerant per-entry and kind-check/spec-parse are deferred to slotted entries, so a broken *unrelated* kinograph entry (non-spec kind, or a fork) no longer fails the file's sync; only the slotted entry's problems are loud. Duplicate resolved names fail loud (`DuplicateEntryName`), matching kinora's ambiguity convention. Pinned-entry naming caveat documented.

67 tests (create/update/unchanged/drift, editable preservation, indentation, pin, unmatched, unslotted, orphans, tolerant unrelated breakage, duplicate names, all error paths). Zero warnings. Unblocks the CLI beans (kinora-exay sync, kinora-guv8 scaffold).
