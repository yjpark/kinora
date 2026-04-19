---
# kinora-q6bo
title: Clean staged after commit; preserve history as kino in 'commits' root
status: draft
type: feature
priority: normal
created_at: 2026-04-19T14:39:21Z
updated_at: 2026-04-19T15:47:21Z
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

## Blocked: design ambiguity

Marked back to draft during night shift after attempting to execute.

**Open design questions that warrant user input:**

1. **Per-root vs per-run archive granularity.** `kinora commit` produces multiple root versions in one run. The bean says archive identity = "commit hash" (singular), but there's no per-run hash today. Per-root (identity = root's new_version hash) is cleaner, but "1:1 with commits" wording is ambiguous.

2. **Recursion: how does the `commits` root commit itself?** Each commit run creates archive kinos assigned to `commits`. Committing `commits` would then consume those assigns and create a new archive, which would be assigned to `commits`, triggering another commit. Either: (a) `commits` is special-cased to skip archive creation for itself, or (b) the commit pipeline walks roots in a fixed order and `commits` last, accepting a one-behind state. Needs a call.

3. **Is `commits` a reserved root or a user-declared root?** Reserved (hardcoded, auto-created) is simpler but less discoverable. User-declared (in config.styx with an auto-bootstrap migration) is more consistent but adds migration work.

4. **`keep-all` policy:** current `RootPolicy` variants (in `config.rs`) may or may not cover this. Needs either a new variant or documentation that existing policies suffice.

5. **Render exclusion signal:** the bean says "like root-kind kinos are skipped". The render layer would need either (a) a config field marking a root as infrastructure, or (b) a hardcoded name check for `commits`. (a) is more extensible; (b) simpler. Needs a call.

Leaving for user to resolve in a follow-up session; proceeding to kinora-b1mg next.
