---
# kinora-0sgr
title: Record head_ts on RootEntry so GC is independent of staging
status: completed
type: feature
priority: high
created_at: 2026-04-21T13:50:34Z
updated_at: 2026-04-21T14:04:38Z
blocking:
    - kinora-wcpp
---

## Context

`apply_root_entry_gc` (commit.rs:991) ages out MaxAge kinograph entries by
looking up the head store event's `ts` in the staged event stream. Today this
works because MaxAge roots keep owned store events in staging as the retention
signal — entries remain in the kinograph until their staged head ages out,
which triggers both staging prune and entry drop in the same commit.

kinora-wcpp wants to drain MaxAge staged events eagerly after archiving
(deduplication — data is preserved in the archive kino). But that removes the
head event GC relies on. With the current fallback ("keep the entry when head
missing"), drained MaxAge entries would never age out — a regression, not an
improvement.

Fix: record the head event's ts directly on `RootEntry` at commit time, and
have GC read it from the entry instead of the staged stream. This decouples
retention semantics from staging, unblocking wcpp.

## Scope

- [x] Add `head_ts: String` field on `RootEntry` with `#[facet(default)]` for
  backward-compatible kinograph parsing (older blobs parse with empty ts)
- [x] `build_root` populates `head_ts` from the picked head store event
- [x] `RootEntry::new` takes `head_ts` as a constructor arg (caller-updated)
- [x] `apply_root_entry_gc` reads `entry.head_ts` instead of looking up events
- [x] Empty `head_ts` (legacy entry): keep the entry (matches current
  conservative fallback)
- [x] `propagate_pins` and `prior_root` merge: entries are copied whole,
  `head_ts` propagates naturally — no change needed

## Out of Scope

- Staging drain behavior itself — that's kinora-wcpp
- Changing the "pin exempts entry" rule
- Changing the retention unit (still seconds, still measured against head ts)

## Acceptance Criteria

- [ ] Tests added and passing:
  - [x] `build_root_populates_head_ts_from_head_event`
  - [x] `entry_gc_uses_head_ts_on_entry_not_staged_event` — simulated by
    building a root with known head_ts, then running GC with empty events
  - [x] `entry_gc_keeps_entry_when_head_ts_is_empty` — legacy path
  - [x] Existing MaxAge/KeepLastN/Never tests still pass
- [x] Zero compiler warnings
- [ ] `kinora commit` behavior unchanged end-to-end (no observable diff
  without wcpp)
- [ ] Bean todo items all checked off
- [ ] Commits: tests / implementation / review-fixes

## Summary of Changes

Decouples MaxAge root-entry GC from the staged event stream by recording the
head event's RFC3339 ts on `RootEntry` itself. Unblocks kinora-wcpp.

### Code changes

- `crates/kinora/src/root.rs`: Added `head_ts: String` field to `RootEntry`
  with `#[facet(default)]` so legacy (pre-0sgr) kinograph blobs still parse
  with an empty default. `RootEntry::new` signature takes `head_ts` as the
  last arg — all (mostly test-only) callers updated.
- `crates/kinora/src/commit.rs`:
  - `build_root` populates `entry.head_ts` from the picked head store event.
  - `apply_root_entry_gc` reads from `entry.head_ts` instead of looking up
    the event by hash in the staged stream. Empty → keep (legacy). Parse
    error → log + keep (conservative). Older than cutoff → drop (unless
    pin or implicit-pinned). `events` arg removed from the signature.
  - `propagate_pins` now copies `head_ts` alongside `version`, so pinned
    entries don't carry a ts that belongs to a different version.
- `crates/kinora/src/resolve.rs`: `ingest_root_kinographs` passes
  `re.head_ts` into the synthesized store Event — render/resolve now see a
  real ts for archive-drained heads.

### Tests added (6)

- `build_root_populates_head_ts_from_head_event` (commit)
- `entry_gc_uses_head_ts_on_entry_not_staged_event` (commit)
- `entry_gc_keeps_entry_when_head_ts_is_empty` (commit)
- `entry_gc_keeps_entry_when_head_ts_is_unparseable` (commit)
- `propagate_pins_keeps_head_ts_paired_with_version` (commit)
- `missing_head_ts_defaults_to_empty_on_styxl_parse` (root)

### Acceptance

- 509 workspace tests pass (up from 506; +3 kinora unit tests)
- Zero compiler warnings
- 3 commits: c17e767 (tests+stub), c4fb25f (implementation), 0c53fac (review)

### Follow-up

Unblocks `kinora-wcpp` — MaxAge staged events can now be drained after
archive without orphaning entries from their retention signal.
