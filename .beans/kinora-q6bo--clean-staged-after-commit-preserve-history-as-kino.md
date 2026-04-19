---
# kinora-q6bo
title: Clean staged after commit; preserve history as kino in 'commits' root
status: in-progress
type: feature
priority: normal
created_at: 2026-04-19T14:39:21Z
updated_at: 2026-04-19T18:18:06Z
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

## Resolved: Q1 archive granularity → per-root

Per-root archive: one archive kino per root per version-bump. Identity = the root's new_version hash (already canonical, no invented run-hash). Each archive's parent is the previous archive for that same root. Mirrors per-branch commit tracking in git.

Per-run queries ("what did this invocation do?") are synthesizable later via `kinora log` — either timestamp-cluster archives, or stamp a shared run-id field into each archive at commit time. Low priority, not in v1.

## Resolved: Q2 commits-root recursion → special-case self-archive

The `commits` root skips archive creation for its own version bumps. The `commits` kinograph already records "archive X added at version V" as its own entries; an archive-of-the-archives would just duplicate that. Staged empties cleanly. Other roots (non-`commits`) still produce archives normally.

Transient assign-events that plumbed archives into `commits` get dropped without archiving — they carry only the archive kino id, which is already recorded in the commits kinograph. (Analog: git reflog vs git commits; reflog is ephemeral by design.)

## Resolved: Q3/Q4/Q5 — mirror the inbox pattern

**Q3 — reserved vs user-declared:** mirror the existing `inbox` pattern. Auto-provision `commits` in `Config::from_styx` if absent (config.rs:146 is the model). Hardcoded by name (`"commits"`) throughout the codebase, same as `"inbox"`. User can override the policy in `config.styx`; user's explicit policy wins. User cannot remove it — library re-injects on load. Visible in config after first serialize.

**Q4 — keep-all policy:** no new `RootPolicy` variant. Reuse existing `RootPolicy::Never` as the default for the `commits` root. Semantically "never drop anything" is exactly what we want for an archive root. Saves churn.

**Q5 — render exclusion signal:** hardcoded name check. `if root_name == "commits" { skip }` in render. Consistent with how infrastructure rules (like the inbox default target) live as hardcoded name lookups today. If a general infra-root marker becomes justified later, migrate the hardcoded checks then. YAGNI now.

## Bean ready to proceed

All 5 open design questions resolved. Implementation can proceed:

1. Q1 decided: per-root archive kinos, identity = root's new_version hash
2. Q2 decided: `commits` root special-cases skip self-archive
3. Q3 decided: auto-provision in `Config::from_styx`, hardcoded by name
4. Q4 decided: reuse `RootPolicy::Never`
5. Q5 decided: hardcoded name check in render

Bean is no longer blocked by ambiguity. Still blocked-by kinora-2t6l (rename hot→staged) — though that is completed. Move to `todo` when ready for implementation.

## Plan

Three phases, each TDD cycle (tests → impl → review fix). Complete 1 phase = commit; move on.

### Phase A — config auto-provision
- Tests in config.rs: commits auto-injected with RootPolicy::Never when absent; user override preserved; serializes back cleanly
- Impl: 1-line addition in Config::from_styx after the existing inbox auto-provision

### Phase B — archive serialization format
- New module `kinora::commit_archive`
- Public API:
  - `serialize_archive(events: &[Event]) -> Vec<u8>` — first line is header `{"@schema":"kinora-commit-archive-v1"}`, then one event-json per line
  - `parse_archive(bytes: &[u8]) -> Result<(Header, Vec<Event>), ArchiveError>`
- Tests: round-trip, empty events, schema header rejection for unknown version

### Phase C — wire it all together
- commit.rs `commit_root_with_refs` after successful pointer write:
  - Skip if root_name == "commits"
  - Collect events consumed by this root commit (those present in this root's rebuild path)
  - Serialize via Phase B
  - Write archive blob to ContentStore with kind="commit-archive"
  - Create AssignEvent with target=archive hash into root="commits"
  - Delete consumed staged event files (existing prune_staged_events logic needs re-gating — today RootPolicy::Never means "keep forever", new lifecycle means "cleaned via archive")
- commit_all: ensure "commits" iterates last (sort with commits pulled to end)
- commit.rs for commits root: consume staged archive-assigns + delete them, skip archive-of-archive creation
- render.rs: `if root_name == "commits" { continue }`
- Tests: full end-to-end (commit consumes → archive stored → commits root receives assign → second commit on commits consumes its staged)

### Todos (replacing original)

- [x] Phase A: config auto-provision tests
- [x] Phase A: config auto-provision impl
- [ ] Phase B: commit_archive module tests
- [ ] Phase B: commit_archive module impl
- [ ] Phase C: integration tests
- [ ] Phase C: commit pipeline archive + cleanup + special-case
- [ ] Phase C: render exclusion
- [ ] Review pass
