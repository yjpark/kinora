---
# kinora-bayr
title: Prune staged events after archive for 'Never' policy roots
status: todo
type: feature
created_at: 2026-04-19T18:41:54Z
updated_at: 2026-04-19T18:41:54Z
---

Per-commit archive kinos now capture provenance for non-commits roots, but staged events still accumulate because RootPolicy::Never (commits + any user-declared Never root) leaves prune_staged_events a no-op. Once we're confident in archive correctness, prune owned staged events after a successful archive: for non-commits roots, drop the events that went into the archive; for the commits root, drop archive-assigns it consumed. Needs merge_prior_unpinned_entries logic so build_root still sees kinos that were archived-and-pruned across subsequent commits.
