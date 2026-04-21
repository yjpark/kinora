---
# kinora-bayr
title: Prune staged events after archive for 'Never' policy roots
status: in-progress
type: feature
priority: normal
created_at: 2026-04-19T18:41:54Z
updated_at: 2026-04-21T01:09:44Z
---

Per-commit archive kinos now capture provenance for non-commits roots, but staged events still accumulate because RootPolicy::Never (commits + any user-declared Never root) leaves prune_staged_events a no-op. Once we're confident in archive correctness, prune owned staged events after a successful archive: for non-commits roots, drop the events that went into the archive; for the commits root, drop archive-assigns it consumed. Needs merge_prior_unpinned_entries logic so build_root still sees kinos that were archived-and-pruned across subsequent commits.


## Night shift 2026-04-20 handoff

Scope investigation confirms this is a medium refactor (~300-500 LOC) touching the core commit module. Scope below is from a fresh-eye subagent exploration.

### Current state

- `prune_staged_events` early-returns for `RootPolicy::Never`, so nothing is pruned for the commits root or any user-declared Never root.
- `build_root` is stateless — it reconstructs a root kinograph from the staged event stream alone. It does not look at the prior root kinograph for unpinned entries.
- `maybe_archive_owned_events` in `commit_archive.rs` already tracks the exact event set that went into each archive — no new plumbing needed to know what to prune.
- No existing tests exercise the archive → prune → rebuild sequence.

### Implementation plan

1. **Extend `build_root` signature** to accept `prior_root: Option<&RootKinograph>`. Add a `merge_prior_unpinned_entries` step that copies kino entries from the prior root whose store events have been pruned from staging (only one non-test caller at `commit.rs:606` needs updating).
2. **Add post-archive prune** in `commit_root_with_refs` for the Never branch: after a successful archive, drop the archived store events from staging. For the `commits` root, drop the archive-assigns it consumed during its commit.
3. **Tests**:
   - archive → prune → rebuild retains kino entries via prior-root merge
   - commits root after archive has staging drained of consumed archive-assigns
   - non-Never policies unchanged (regression guard)
   - pin flags from prior root still propagate correctly when store events are pruned (no prior-root entry has a live staged head)
   - cross-root implicit pin still protects entries whose store events have been archived

### Risk callouts

- `build_root` is `pub`; its callers in tests (commit.rs:1590 and elsewhere) need to be updated to pass `None` for `prior_root`.
- The merge has to handle the case where the prior root entry's content hash is no longer in the store — either leave it or assert reachability.
- Archived-events-by-id tracking needs to be threaded from `maybe_archive_owned_events` back into the prune step so we drop exactly what was archived.

Deferred because this deserves a dedicated session with room for careful design; not a fit for end-of-night-shift.

## Plan (2026-04-21 night shift)

Implementation based on the handoff plan, with concrete function shapes.

### Signature change

`build_root(events, root_name, declared_roots)` → `build_root(events, root_name, declared_roots, prior_root: Option<&RootKinograph>)`.

After the current staged-events-based construction, if `prior_root` is present, merge any entry from `prior_root` whose `id` is NOT already in the fresh build AND which has no live-assign reassignment away from `root_name`. Preserves pin + version + note verbatim.

Rationale for the `reassigned-away` check: when all owned staged events have been pruned for a kino that was reassigned to another root, the only signal that survives is the new assign. Without the check, merge would resurrect the old entry under this root after the kino has moved.

### Commit path change

`maybe_archive_owned_events` returns `Option<(archive_id, archived_event_hashes)>` instead of `Option<archive_id>`. Non-None when owned events existed (whether archive kino is newly stored or pre-existed from crash replay).

In `commit_root_with_refs`, split the Never-path prune out of `prune_staged_events`:

- Non-commits root, new_version emitted, Never policy: after archive, drop the archived event hashes via new `drop_staged_events`.
- Commits root, new_version emitted, Never policy: compute owned staged events (archive store events + archive-assigns routed here), drop them.
- Other policies: unchanged — `prune_staged_events` still handles MaxAge/KeepLastN.

### Tests

1. `never_policy_drains_staged_after_archive` — commit a Never root with kinos; after commit, staged dir for this root's events is empty; archive blob exists; root kinograph has the entries.
2. `commits_root_drains_archive_assigns_and_stores` — commit_all; after, commits root's owned staged events are gone; commits kinograph has archive entries.
3. `never_policy_rebuild_preserves_prior_entries` — commit Never root; no new events added; commit again → no-op (would otherwise be empty).
4. `never_policy_merge_respects_reassignment` — commit Never root with kino A; prune drops A; user reassigns A to `second-root`; commit Never root again → A no longer in Never root's kinograph.
5. `build_root_prior_merge_preserves_pin` — unit test against `build_root` directly with a synthetic prior_root containing a pinned entry and no staged store events for its id; result includes the entry with pin=true.
6. `maxage_policy_unchanged_regression` — existing MaxAge tests should stay green (covered by existing suite).

## Decisions

- **Literal bean wording "drop archive-assigns it consumed" interpreted broadly:** drop ALL owned staged events on the commits root (archive store events + archive-assigns). Rationale: the bean's motivating problem ("staged events still accumulate") isn't fully solved by dropping only assigns. Consistent with non-commits Never behavior.
- **Missing content blob on prior-root merge:** include the entry regardless. Blob reachability is a separate concern (fsck-like); not the rebuild's job.
- **Keep `propagate_pins` AND add merge:** complementary paths — pins propagate into entries that exist in the fresh build, merge adds entries that do not.
