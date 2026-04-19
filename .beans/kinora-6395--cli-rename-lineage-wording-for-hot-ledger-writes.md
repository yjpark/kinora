---
# kinora-6395
title: 'CLI: rename `lineage=…` wording for hot-ledger writes'
status: completed
type: task
priority: low
created_at: 2026-04-19T06:23:57Z
updated_at: 2026-04-19T07:20:03Z
parent: kinora-w7w0
---

On every successful `kinora store`, the CLI prints something like:

    lineage=a1b2c3d4 (new lineage)

Under the hot-ledger layout, each event lives in its own file keyed by the event hash — there is no 'lineage file' anymore, and every hot write is trivially 'new'. The message is a carryover from the old per-lineage ledger layout and is now misleading:

- Re-storing the same logical event (idempotent no-op) still prints `(new lineage)` if the shorthash happens to differ from a prior run's print — but actually, on idempotent re-store it now prints nothing of note since we suppress; but on any new event (even a version under an existing identity) we say 'new lineage' even though the identity is unchanged
- New users map 'lineage' to a branch-like concept; the shorthash is really just the event hash's prefix

Observed while completing kinora-ve9g.

## Proposal (sketch)

- Rename the printed field to `event` (or `eh` for 'event hash') — e.g. `event=a1b2c3d4`
- Drop `(new lineage)`; use `(new event)` when `was_new_lineage=true` and omit otherwise
- Keep the `StoredKino.lineage` field name as-is for one release so programmatic callers aren't broken — document the deprecation in code

## Acceptance

- [x] CLI print updated
- [x] Integration/unit test asserts the new wording
- [x] Docs/README reflects the new phrasing if it appears anywhere (README/docs checked — no occurrences)

## Summary of Changes

- CLI print for successful stores reworded from `lineage=<sh> (new lineage)` to `stored kind=<k> id=<id> hash=<h> event=<sh> (new event)`.
- `(new event)` suffix only appears when `was_new_lineage=true` (i.e. the hot-ledger file was actually created); idempotent re-stores print no suffix.
- Formatter extracted as `format_store_summary(&StoredKino) -> String` in `crates/kinora-cli/src/store.rs` so it can be unit-tested without running a real store.
- Three new tests pin the wording: `format_store_summary_uses_event_wording_for_new_events`, `format_store_summary_omits_suffix_on_idempotent_restore`, `format_store_summary_has_expected_shape`.
- `StoredKino.lineage` and `StoredKino.was_new_lineage` field names kept as-is for one release so programmatic callers (and their tests) aren't broken; deprecation documented inline in `crates/kinora/src/kino.rs`.
- No docs/README changes needed — neither mentions "lineage" outside this bean's own history.
