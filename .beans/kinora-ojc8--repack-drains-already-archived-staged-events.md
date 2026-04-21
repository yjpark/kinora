---
# kinora-ojc8
title: Repack drains already-archived staged events
status: in-progress
type: bug
priority: normal
created_at: 2026-04-21T14:44:31Z
updated_at: 2026-04-21T14:47:15Z
---

Close the migration-debt gap from wcpp: repack should drain staged events whose hashes already exist in a commit-archive kino.

## Context

kinora-wcpp's drain fires only when `result.new_version.is_some()` (see
`commit.rs:771-783`). Repos with pre-wcpp staging — where events were
archived into `commit-archive` kinos but never drained (MaxAge roots
pre-wcpp were gated off) — can't self-clean via a no-op commit.

`clone_repo` (which repack invokes after commit) copies staged events
wholesale as long as their content blobs are reachable. Since the blobs
are reachable (archive kinos reference them, and the root kinograph
entries pin their versions), those stale staged events survive
the clone. Repack completes, `.kinora/staged/` stays populated.

## Scope

Add a post-commit, pre-clone drain pass inside `repack_repo` that
drops staged events whose hash already appears in a `commit-archive`
kino, for source roots with `RootPolicy::Never | MaxAge(_)`.

- [ ] Add `drain_archived_orphans(kinora_root) -> Result<usize, CommitError>`
  in `commit.rs`:
  - Load config + commits root kinograph
  - For each commits-root entry of kind `commit-archive`, parse
    metadata `name` (`<source_root>-commit-archive`) to identify the
    source root
  - If `source_root`'s policy is `Never | MaxAge(_)`, read the archive
    blob from ContentStore, parse it, collect event hashes, and
    `drop_staged_events` those hashes
  - Tolerate missing `commits` pointer (fresh repos, empty archives)
- [ ] Call it from `repack.rs::repack_repo` after `commit_all` and
  before `clone_repo`
- [ ] Surface the drained-count in the `RepackReport` (new
  `orphan_events_drained: usize` field) and in CLI output
- [ ] Tests:
  - [ ] Migration-debt scenario: simulate pre-wcpp state by
    re-writing a staged event that's already in an archive;
    repack drains it
  - [ ] No-op scenario: staged event not in any archive stays put
  - [ ] Policy gate: KeepLastN source root's archived events are
    NOT drained (KeepLastN keeps its retention in staging)

## Acceptance

- [ ] Tests added and passing
- [ ] No regression of existing repack/commit tests
- [ ] Zero compiler warnings
