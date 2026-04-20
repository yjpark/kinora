---
# kinora-bayr
title: Prune staged events after archive for 'Never' policy roots
status: todo
type: feature
priority: normal
created_at: 2026-04-19T18:41:54Z
updated_at: 2026-04-20T13:59:42Z
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
