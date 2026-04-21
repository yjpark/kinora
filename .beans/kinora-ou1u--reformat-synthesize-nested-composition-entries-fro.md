---
# kinora-ou1u
title: 'Reformat: synthesize nested composition entries from pins'
status: completed
type: task
priority: low
created_at: 2026-04-21T14:36:18Z
updated_at: 2026-04-21T15:14:05Z
---

## Context

kinora-wcpp (completed 2026-04-21) widened the post-archive drain to
`MaxAge`, which means most kinograph store events in the default
`inbox` root (MaxAge 30d) are drained from staging after each commit.
They remain recoverable via the `commit-archive` kino.

wcpp patched `reformat_repo` Step 2 to synthesize store-event stubs
from **root kinograph entries** so `pick_head` can resolve archived
heads. That covers the common case: user runs `reformat` against a
repo where top-level entries sit in root kinographs.

**Gap:** nested composition entries — kinograph entries referenced by
another kinograph's `entries` list, that are NOT themselves root
kinograph entries — have no synthesis path. When `pick_head` tries
to resolve such an id, `events_by_id.get(&id)` returns None and the
entry is silently skipped (reformat.rs:~233-235).

Pre-existing for Never-policy roots, but now visible on every MaxAge
root (which is most of them in practice).

## Why This Is Low-Priority

- Reformat is idempotent and runs against a live repo. A subsequent
  `commit` cycle re-surfaces nested kinographs by materializing them
  as new kinograph store events (either via user edits or normal
  churn), at which point the next `reformat` run catches them.
- Legacy (.styx-wrapped) kinographs are only produced by pre-styxl
  versions of kinora. The population that needs this migration is
  fixed and small; once any given repo has reformatted its heads
  once, the gap only matters for never-reformatted nested kinographs.
- The drained event's content still exists in the archive kino; the
  information isn't lost, just not *reformatted* on the first pass.

## Scope

Close the gap by threading synthesis through composition entries:

- [x] Extend `to_visit` (reformat.rs:201) to carry `(id, version)`
  pairs — the version comes from a root entry's `version` or a
  composition entry's `pin`.
- [x] When visiting a nested entry, synthesize a stub into
  `events_by_id` using the pin (if present) as the version, read
  the content from ContentStore to determine kind, and fall through
  to the existing `pick_head`-free resolution path.
- [x] When a composition entry has no pin AND its store event is
  neither staged nor in any root kinograph, keep the silent skip
  (nothing to do). Debug log not added — the silent skip was already
  in place and there was no observability need surfaced.
- [x] Add a test: nested kinograph with a drained store event whose
  parent is a root-entry kinograph — reformat should produce a new
  version for both the parent and the nested entry.

## Acceptance

- [x] Test added and passing.
- [x] No regression of existing reformat tests.
- [x] Zero compiler warnings.

## Plan

- Change `to_visit: Vec<String>` to `Vec<(String, Option<String>)>` — second field is the version hint (from root entry's `version` or a composition entry's `pin_opt()`).
- In the root-kg seed loop, push `(entry.id, Some(entry.version.clone()))`.
- In the walk, when `events_by_id.get(&id)` is `None`:
  - If `hint` is `Some`, read that hash's content from the ContentStore, try `Kinograph::parse` — if it parses, synthesize a kinograph-kind Event stub and insert into `events_by_id`.
  - If `hint` is `None`, or content doesn't parse as a kinograph, `continue` (the pre-existing silent-skip).
- When recursing into composition entries, push `(entry.id, entry.pin_opt().map(ToOwned::to_owned))` — pin becomes the hint for nested resolution.
- Test: construct a repo where inner's store event is drained from staging and inner is not a root kg entry, but outer (which IS a root entry) composes inner with `pin = inner.hash`. Reformat should produce new versions for both outer and inner.

## Summary of Changes

Closed the wcpp migration-debt gap for nested kinograph entries. Before: a kinograph referenced only via another kinograph's composition entry list (not itself a root entry) whose store event was drained from staging would silently skip reformat. Now: if the composition entry carries a `pin`, reformat uses it as a version hint to synthesize a stub and reformat the inner kinograph on the same pass.

### Implementation (`crates/kinora/src/reformat.rs`)

- Changed `to_visit: Vec<String>` → `Vec<(String, Option<String>)>` — second element is the version hint.
- Root-entry seed pushes `(id, Some(entry.version))`.
- Composition-entry recursion propagates `entry.pin_opt().map(ToOwned::to_owned)`.
- New synth-from-hint block inside the walk reads the hint hash, verifies it parses as `Kinograph`, and injects a kinograph-kind stub into `events_by_id`. Handles `StoreError::Io(NotFound)` as silent skip (reaped blob); all other store errors propagate.

### Tests (3 new)

- `reformat_resolves_nested_pin_when_store_event_drained` — the main fix: outer (root entry) + inner (drained, nested-only) both reformat via pin synthesis.
- `reformat_silently_skips_nested_entry_without_pin_when_drained` — no-hint silent-skip contract.
- `reformat_silently_skips_nested_pin_pointing_at_non_kinograph_content` — defends against synthesizing a kinograph-kind stub from opaque content (e.g. markdown).

### Results

- 522 tests pass, zero compiler warnings
- Commits: dcc8deb (test), c699f88 (impl), 243a669 (review fixes)

### Deviation from plan

Plan mentioned a debug log for the no-pin-no-event silent skip. Didn't add one — the silent skip was pre-existing and nothing surfaced a concrete observability need. Can be revisited if operators later find reformat runs finishing "too clean".
