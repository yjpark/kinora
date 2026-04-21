---
# kinora-wcpp
title: Drain archived events from staging for MaxAge (generalize bayr)
status: completed
type: feature
priority: high
created_at: 2026-04-21T13:47:30Z
updated_at: 2026-04-21T14:33:29Z
---

## Context

kinora-q6bo introduced the commit-archive (staged events archived into a
root-specific `commit-archive` kino). kinora-bayr added post-archive staging
drain + `prior_root` merge in `build_root`, but gated both on
`RootPolicy::Never` only.

Result: roots configured with `MaxAge(duration)` (including the
auto-provisioned `inbox` root with `MaxAge("30d")`) still accumulate committed
events in `.kinora/staged/` indefinitely. Users running `kinora commit` +
`kinora repack` expect staging to be clean — but only Never-policy roots
benefit today.

The archive already preserves the data, so removing staged events after
archiving is pure deduplication, not data loss — this is what "commit"
conceptually means.

## Scope

Extend bayr's drain machinery to also fire for `MaxAge`:

- [x] `commit_root_with_refs` — drop the `RootPolicy::Never` gate so
  `drain_archived_events_from_staging` also runs for `MaxAge`
- [x] `build_root` — extend the `prior_root` merge path to `MaxAge` so old
  entries survive across commits and only age out via
  `apply_root_entry_gc`
- [x] `prune_staged_events` — remove the `MaxAge` branch (retention now lives
  in the root kinograph via `apply_root_entry_gc`, not in staging)
- [x] Keep `KeepLastN` untouched — its "N versions per kino" semantic is
  load-bearing on staging (root kinograph is one-entry-per-id). A separate
  bean can revisit this later if needed.

## Acceptance Criteria

- [x] Tests added and passing:
  - [x] `max_age_drains_archived_events_from_staging_after_commit`
  - [x] `max_age_entries_age_out_of_root_kinograph_via_gc_post_drain`
  - [x] `max_age_prior_root_merges_entries_across_commits`
  - [x] Never-policy regression test remains green
  - [x] `KeepLastN` regression test remains green (retention-via-staging
    preserved)
- [x] Zero compiler warnings
- [x] `kinora commit` + `kinora repack` on a default `inbox` root leaves
  `.kinora/staged/` empty (code-level; manual verify deferred)
- [x] Bean todo items all checked off
- [x] Commits: tests / implementation / review-fixes

## Summary of Changes

Generalized the bayr drain + `prior_root` merge machinery from `RootPolicy::Never` alone to `Never | MaxAge(_)`. MaxAge retention now lives entirely on the root kinograph via `apply_root_entry_gc` (using `head_ts` per entry from kinora-0sgr); staging no longer carries the retention signal.

### Code changes

- `crates/kinora/src/commit.rs`:
  - `build_root` merge_source gate widened to `Never | MaxAge(_)`: prior_root merges entries across commits so drained events don't orphan their root entries.
  - Post-archive drain gate at the non-commits branch widened to `Never | MaxAge(_)`: archived events drop from staging.
  - Commits-root drain gate also widened to `Never | MaxAge(_)`: protects against leak when a user configures `commits` with non-default policy.
  - `prune_staged_events` now early-returns for `Never | MaxAge(_)`; the outer match collapsed to an `if let RootPolicy::KeepLastN(n)` let-else, dropping ~80 lines of now-unreachable MaxAge code, the `now` parameter, and `owned_assigns` collection.
  - `propagate_pins`: unchanged (already correctly copies head_ts from prior via kinora-0sgr review fix).
- `crates/kinora/src/reformat.rs`:
  - Step 2 now synthesizes store-event stubs from root kinograph entries for ids absent from staging — so `pick_head` resolves archived heads. Without this, reformat_repo silently skipped every kinograph whose store event had been drained.
  - Documented the nested-composition gap (archived-only nested kinographs aren't reformatted first-pass; next commit cycle re-surfaces them).

### Tests added (3) and adapted (5)

New (commit.rs):
- `max_age_drains_archived_events_from_staging_after_commit`
- `max_age_entries_age_out_of_root_kinograph_via_gc_post_drain`
- `max_age_prior_root_merges_entries_across_commits`

Adapted for wcpp semantics:
- `max_age_drains_both_old_and_fresh_owned_events` (was `max_age_staged_ledger_prunes_events_older_than_policy`) — drain is now both fresh + old, not cutoff-gated.
- `max_age_drains_both_old_and_fresh_assign_events` (was `max_age_prunes_old_assign_events_too`) — same rationale.
- `fresh_staged_events_untouched_by_keep_last_n_policy` (was `fresh_staged_events_untouched_by_policy`) — switched to keep-last-10 config, since MaxAge no longer keeps anything in staging.
- `cross_root_ref_from_a_prevents_b_gc_from_dropping_referenced_version`: removed the `staged_event_exists` assertion. The staged event is drained by wcpp, but the root entry survives via prior_root merge + implicit-pin GC — that is cross-root integrity post-wcpp.
- `removing_cross_root_reference_allows_subsequent_gc_drop`: added a fresh kg_v2→rfcs assign. Pre-wcpp the test passed via an assign-aging quirk (old assigns dropped from staging even when their store events were pin-protected, defaulting the store to inbox). Post-wcpp, cross-root integrity correctly protects via prior_root merge; need an actual new assign to rotate rfcs v2's reference from X to Y.
- `reformat_skips_markdown_and_text_kinos`: switched from counting total staged versions of md/text (now 0 after archive) to counting NEW versions staged by reformat (should be 0).

### Acceptance

- 512 workspace tests pass (up from 509 on kinora-0sgr completion; +3 kinora unit tests)
- Zero compiler warnings
- 3 commits: 5c80148 (tests), 9ea15a9 (implementation), e76cddb (review fixes)

### Follow-up

Nested-composition reformat gap: when a nested kinograph's store event is drained and it is not directly a root entry, reformat silently skips it. Pre-existing for Never-policy roots; more visible under MaxAge. A follow-up bean could synthesize from composition-entry pins to close this, but reformat is idempotent — a subsequent commit cycle re-surfaces missed entries.
