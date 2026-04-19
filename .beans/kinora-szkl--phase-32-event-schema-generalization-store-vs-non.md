---
# kinora-szkl
title: 'Phase 3.2: event schema generalization (store vs non-store events)'
status: completed
type: task
priority: normal
created_at: 2026-04-19T10:16:54Z
updated_at: 2026-04-19T11:07:48Z
parent: kinora-hxmw
---

Add event-kind discriminator to hot-ledger events; keep legacy store events parsing unchanged.

Second piece of phase 3 (kinora-hxmw). Today the hot-ledger `Event` struct assumes every event is a store event (content hash, kind-as-blob-kind, parents). Phase 3 needs multiple event kinds (`store`, `assign`, and future kinds like metadata resolution). This bean introduces the discriminator and refactors the consumers; no new event type lands here — that's hxmw-3.

## Scope

### In scope

- [x] Refactor the on-disk hot event shape to include an event-kind discriminator distinct from the content `kind` field. **Chosen shape**: flat `event_kind: String` field as the first field of `Event` (not a tagged enum). See Summary.
- [x] Legacy hot files (no discriminator) parse as Store events (via narrow fallback triggered only on "missing field `event_kind`" primary error).
- [x] Update `Ledger::read_all_events` (and related callers) to return the generalized shape — `Event` struct now carries `event_kind`; `read_all_events` signature unchanged.
- [x] Update consumers that iterate store-specific data to filter for store events: `Resolver::load` (both legacy-lineage and hot loops) and `compact::build_root` now call `is_store_event()`. Render is downstream of resolver so inherits the filter; validate operates per-event on store-constructed events only.
- [x] Event-hash computation stays deterministic per kind — `event_kind` participates in canonical serialization.
- [x] `.jsonl` file path continues to be `<event-hash>` based for new events; legacy files keep their original paths (documented in `from_json_line`).
- [x] Tests: round-trip (passes), legacy parse (passes), non-store ignored by resolver + compact (passes), malformed new-shape event does not fall back (passes).

### Out of scope (deferred)

- The `assign` event type itself (hxmw-3)
- `kinora assign` / `kinora store --root` CLI (hxmw-3)
- Compaction consuming non-store events (hxmw-5)

## Acceptance

- [ ] All sub-points under "In scope" implemented with tests
- [ ] Zero compiler warnings
- [ ] Bean todo items all checked off
- [ ] Summary of Changes section added at completion (including the chosen on-disk shape)

## Summary of Changes

### Chosen shape

Flat `event_kind: String` field, first in the struct (JSON line begins with `{"event_kind":...}`). Preferred over a tagged-enum `HotEvent { Store(_), Assign(_) }` because:
- facet_json round-trip is frictionless for strings; tagged enums would require a custom derive path
- The existing `Event` struct stays single-typed — consumers that need to distinguish call `is_store_event()`; consumers that don't care (dedup by hash, json round-trip) see one shape
- Future kinds (`assign`, `metadata-resolve`) slot in without a type-system refactor

### Code

- `crates/kinora/src/event.rs`: new `event_kind` field, `EVENT_KIND_STORE` constant, `Event::new_store(...)` ctor, `is_store_event()` predicate. Added `LegacyEvent` private struct and a narrow fallback in `from_json_line` that only triggers when the primary parse error is "missing field `event_kind`". Documented the hash-identity semantics: promoted legacy events' `event_hash()` uses the new canonical form, so `file_path == event_hash` only holds for events written post-phase-3.
- `crates/kinora/src/resolve.rs`: filter on `is_store_event()` in both legacy-lineage and hot-event loops.
- `crates/kinora/src/compact.rs`: filter on `is_store_event()` in `build_root`.
- Existing call sites (`kino.rs`, `ledger.rs`, `validate.rs`, `resolve.rs` tests, `compact.rs` tests, `kinora-cli` tests) migrated to `Event::new_store(...)` or explicit `event_kind` field.

### Tests

4 new event tests (round-trip incl. event_kind, new_store ctor, non-store predicate, event_kind vs content kind, legacy parse, malformed-new-shape-no-fallback) plus consumer filter tests in resolve + compact. Full workspace green: 303 tests.

### Deferred follow-ups

Cross-layout dedup between legacy (on-disk hash = legacy-form) and hot (hash = new-form) is imperfect if someone re-writes a legacy event with `write_event` — forks it. No current code path does this; if a migration tool is ever introduced, it must either delete the legacy file or emit events at both paths.

### Commits

- a941093 test(event): event_kind discriminator + consumer filter tests
- 550686c feat(event): legacy parse fallback + store-event filter in consumers
- 4380f7a fix(event): narrow legacy fallback + document hash identity
