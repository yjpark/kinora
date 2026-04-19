---
# kinora-b1mg
title: 'kinora clone: rebuild .kinora/ into a fresh directory'
status: todo
type: feature
priority: normal
created_at: 2026-04-19T14:51:05Z
updated_at: 2026-04-19T16:33:49Z
blocked_by:
    - kinora-jezf
    - kinora-q6bo
---

## Why

After a few days of development `.kinora/` already carries legacy: store blobs without extensions (pre-kinora-wpup), possibly other lingering artifacts from old designs. A *rebuild* primitive lets us sanitize the data by re-running it through the current write path, rather than hand-rolling migrations for each format change.

Clone is also the atomic unit the upcoming `kinora repack` command will compose (commit → clone → swap).

## CLI shape

```
kinora clone <src> <dst>
```

- Args are **direct paths to `.kinora/` directories**, not to enclosing repo roots. Clone does not walk up looking for a repo; it does not require being inside a repo.
- Typical invocation: `kinora clone .kinora .kinora2`
- Dst must not exist (or must be empty). Error if dst already contains content.

## Semantics — rebuild, not copy

1. **Validate src**: staged ledger must be empty. If non-empty, error with a message pointing users to `kinora commit` first. This keeps the contract crisp: *clone operates on committed state only.*
2. **Enumerate reachable content**: for each root, walk its root-kinograph heads → traverse parent chains → collect the set of reachable kino hashes.
3. **Re-store reachable kinos**: for each reachable hash, read content from src store (tolerating both legacy `<hash>` and current `<hash>.<ext>` filenames), then write to dst store through the current store API. Filenames come out in the current convention automatically.
4. **Rebuild root-kinographs**: write fresh root-kinograph files using the current format, from the reachable graph.
5. **Staged ledger**: dst has an empty `.kinora/staged/` (or equivalent post-rename).
6. **Unreachable blobs in src**: dropped. This is the cleanup payoff.
7. **Hash verification**: while reading each src blob, verify its content hashes to the claimed name. On mismatch, abort with a clear error (corruption is not something clone should silently paper over).

## Abstracting path assumptions

This is a good forcing function to push file-path assumptions down into the store layer. Today callers build `<hash>.<ext>` paths in multiple places; clone should go through a single "read blob by hash" API that tolerates legacy filenames, and a "write blob" API that emits current format. Future pack-file support (see memory: planned pack-file format) will slide in behind the same API.

## Output

Summary on stdout:
- N kinos rebuilt
- M blobs dropped (unreachable)
- K filenames rewritten (legacy → current convention)

## Depends on

- `--repo-root` flag bean (so clone integrates cleanly with path plumbing)
- Rename bean (hot→staged, compact→commit) — *not strictly required*, but if it lands first the staged-empty check uses the new name natively

## Design decisions

- **Clone is hash-preserving.** It rewrites filenames to current conventions and drops unreachable blobs, but does NOT re-serialize or migrate content. Content migration (e.g. styx single-line → multiline) is `kinora reformat`'s job. Two atomic operations kept separate.
- **Atomicity**: clone writes to dst incrementally. If it fails midway, dst is left partial. Acceptable — dst is a throwaway location by convention; repack handles atomicity at the swap step.

## Acceptance

- `kinora clone .kinora .kinora2` produces a dst that `kinora render`/`resolve` treats identically to src
- Rebuilt dst has zero blobs without extensions (post kinora-wpup naming)
- Rebuilt dst drops unreachable blobs from src
- Rebuilt dst has empty staged
- Src is unchanged
- Clone errors cleanly if src has dirty staged, with a pointer to `kinora commit`
- Hash mismatch in src surfaces as a clear error
- Zero warnings, all tests pass

## Blocked: implicitly depends on kinora-q6bo

Attempted to execute during night shift; hit a spec contradiction that can't be resolved without q6bo (staged-cleanup-after-commit) landing first.

**The contradiction:**
- Bean step 1: "staged ledger must be empty. If non-empty, error."
- Bean step 5: "Staged ledger: dst has an empty `.kinora/staged/`"
- Today (pre-q6bo), every event — committed or not — lives permanently in `.kinora/staged/`. There is no path that produces an empty staged. Every repo is "dirty" by the bean's contract.

**Why we can't just drop the empty-staged check:**
- If clone ignores staged, dst has no events → `Resolver::load` returns empty → `kinora resolve` and `kinora render` on dst don't work → breaks acceptance criterion "produces a dst that kinora render/resolve treats identically to src".
- If clone copies staged events verbatim, dst's staged is not empty → contradicts step 5.
- The bean assumes a post-q6bo world where staged is transient and ledger (or the new `commits` root archive) holds the authoritative history. In that world, clone copies the committed archive, leaves staged empty, and resolver walks the archive.

**Resolution:** landing q6bo first unblocks b1mg naturally — the commit lifecycle will have moved history out of staged into a stable location clone can copy. Marking this bean blocked-by kinora-q6bo and draft until the q6bo design questions are resolved.

Also amending the bean spec here implicitly: b1mg's "depends on" section originally said q6bo was *not strictly required*. After attempting execution I disagree — it IS strictly required, for the reason above.
