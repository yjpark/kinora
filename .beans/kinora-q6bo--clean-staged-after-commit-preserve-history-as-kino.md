---
# kinora-q6bo
title: Clean staged after commit; preserve history as kino in 'commits' root
status: todo
type: feature
priority: normal
created_at: 2026-04-19T14:39:21Z
updated_at: 2026-04-19T15:29:55Z
blocked_by:
    - kinora-2t6l
---

## Why

Today, events in the hot/staged ledger are never cleaned. Over time this grows unboundedly and makes 'staged' a misnomer — it's really 'staged + archived'.

Cleaning staged after commit matches the mental model users bring from git, but loses per-event provenance that is currently addressable (hash → assign event metadata). We want the trail to survive so that `kinora resolve` or a future `kinora log` can reach back into it for auditing or debugging.

## Approach

After a successful commit:

1. Collect the ordered list of staged events consumed by this commit (per root).
2. Serialize them as a single linear blob — proposal: newline-delimited JSON, same shape as today's `.kinora/staged/<ab>/<hash>.jsonl` entries, concatenated in commit order.
3. Store that blob as a kino under a reserved root (proposed name: `commits` or `history`). The kino's content is the archive; its identity/name is the commit hash it belongs to.
4. The `commits` root has a `keep-all` policy so archives are never GC'd.
5. Default render skips `commits`-root kinos the way `root`-kind kinos are already skipped — they're infrastructure, not user content.
6. Remove the consumed event files from `.kinora/staged/`.

## Design decisions

- **Root name**: `commits` (1:1 with commits).
- **Archive kino identity**: plain commit hash. The root is already encoded in the archive's content.
- **Archive format**: JSONL with a single-line header row containing schema version. Cheap future-proofing — schema version lets future readers migrate or reject old archives; body stays greppable.
- **Partial-failure recovery**: archive first, then delete staged events. Re-running commit detects `archive already exists for this commit` (content-addressed — this is automatic via the store) and resumes the cleanup step idempotently.
- **Resolve UX**: dedicated `kinora log <commit-hash>` command in a follow-up bean. `kinora resolve` returning a ledger archive is off-model (resolve returns user content by identity, not commit metadata).

## Depends on

- Rename bean (hot→staged, compact→commit) should land first so this is just the lifecycle change, not rename + lifecycle tangled.

## Acceptance

- After `kinora commit`, `.kinora/staged/` is empty of committed events
- Archive kino is stored under the reserved root with `keep-all` policy
- Default render ignores the archive root
- Tests cover: clean-after-commit, archive content matches consumed events in order, partial-failure recovery
- Zero compiler warnings
