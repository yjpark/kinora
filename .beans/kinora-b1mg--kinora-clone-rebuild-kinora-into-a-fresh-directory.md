---
# kinora-b1mg
title: 'kinora clone: rebuild .kinora/ into a fresh directory'
status: completed
type: feature
priority: normal
created_at: 2026-04-19T14:51:05Z
updated_at: 2026-04-20T13:44:26Z
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

## Plan

Three phases, each TDD (tests → impl → review fix per CLAUDE.md).

### Phase A — library `kinora::clone` module

Module API (mirrors `reformat.rs` structure):

```rust
pub struct CloneParams { pub author: String, pub provenance: String, pub ts: String }
pub struct CloneReport {
    pub kinos_rebuilt: usize,
    pub blobs_dropped: usize,
    pub filenames_rewritten: usize,
}
pub fn clone_repo(src: &Path, dst: &Path, params: CloneParams) -> Result<CloneReport, CloneError>
```

Algorithm:
1. Validate src: staged dir is empty (no pending `.jsonl` files)
2. Validate dst: either doesn't exist or is empty
3. Init dst layout: config.styx (copy verbatim — not a kino, not content-addressed), ledger/, roots/, HEAD
4. Walk reachable blobs: for each root pointer in src, read root kinograph, collect entry ids + root hash; recurse into kinograph-kind entries' heads
5. For each reachable hash: `src_store.read` (with hash verification already built-in) → `dst_store.write(kind, content)`
6. For each root, copy root pointer to dst (pointing at same blob hash — the blob is already in dst store)
7. Copy ledger events corresponding to reachable blobs
8. Report counts

### Phase B — CLI command `kinora clone <src> <dst>`

Follow the `kinora reformat` / `kinora commit` pattern:
- Add `Clone { src: String, dst: String }` variant to `cli::Command`
- New `kinora-cli/src/clone.rs` with `run_clone(cwd, args)` — both paths are taken verbatim (NOT walked up via find_repo_root — per bean spec, args are direct paths to `.kinora/` directories)
- Wire in `main.rs`
- Output: formatted summary

### Phase C — review fix commit if needed

### Todos

- [x] Phase A: library clone module — tests first (empty repo, one-kino repo, reachable walk, hash verification, staged-non-empty error)
- [x] Phase A: library clone module — impl
- [ ] Phase B: CLI `kinora clone` — tests + impl
- [x] Phase C: review + fixes

## Night shift 2026-04-19 handoff

Plan is captured above. Moved back to `todo` (not `draft` — spec is clear, no open design questions).

Rationale for handoff: this task is a new library module (`kinora::clone`) plus a new CLI command, with a reachability walk, hash verification flow, root pointer + ledger event copy, and acceptance criteria around legacy-filename rewriting. Scope is 3 phases, ~5–8 commits. Better to start it in a fresh session than spill across a compacted one where context has already been heavily used on the preceding kinora-tx3e work.

## Implementation note (2026-04-20 resume)

Pragmatic deviation from bean step 1 ("staged must be empty"): kinora-q6bo landed the archive machinery but deferred staged-cleanup itself to kinora-bayr. So pre-bayr every repo has a populated staged dir, and enforcing staged-empty would make clone impossible today.

Workaround:
- Drop staged-empty check for now
- Filter events during copy: keep store events whose `hash` is in reachable_blobs; keep all non-store events (assigns) verbatim
- Re-instate the staged-empty check once bayr lands; at that point the dst staged-empty acceptance criterion becomes naturally satisfied

Everything else in the bean spec stands.

## Summary of Changes

Landed in four commits on main:

- `5ca661c` test(clone): spec library clone semantics — 12 failing tests + type stubs.
- `ca19a87` feat(clone): walk reachable blobs into fresh dst — full `kinora::clone` module.
- `dac331d` fix(clone): reuse `Ledger::write_event`; cover all-reachable case — Phase A review fixes.
- `67f1951` feat(cli): add `kinora clone` command — Phase B CLI wrapper + Phase C review fixes.

### Design deviations from the bean spec

1. **Staged-empty precondition was dropped.** The spec assumed a post-kinora-q6bo world where staged is transient. Instead of blocking on q6bo, clone now filters events during the copy: store events whose blob hash is reachable are kept; all non-store events (assigns) are kept verbatim. Dst's staged is non-empty but carries only reachable history, which satisfies the acceptance criterion "produces a dst that kinora render/resolve treats identically to src".
2. **Hash verification is inherited from `ContentStore::read`** rather than implemented separately — the store API already verifies hashes on read.
3. **Clone is implemented as a library-level operation that takes direct `.kinora/` paths.** The CLI wrapper is a thin shell; `-C` / `--repo-root` is rejected when combined with clone (hard error, not silent ignore).
4. **Author and provenance default to literal `"clone"`.** Clone doesn't derive author from git — it's a local rebuild that may run outside a git worktree.

### What's covered by tests

- Library: 13 tests covering empty repo, single-kino, composition recursion, unreachable-blob drop, legacy extensionless filename rewrite, hash preservation, staged event filtering, multiple-head error, no-head error, src-invalid / dst-not-empty errors, all-reachable case.
- CLI: 9 tests covering success on empty repo, src-not-kinora error, dst-not-empty error, relative-path resolution, default author/provenance, and summary formatting at 0 / 1 / many counts.

### Not addressed (out of scope)

- `kinora repack` (commit → clone → swap atomic pipeline) — separate bean.
- `ContentStore::read` legacy-name tolerance was already in place from kinora-wpup; clone just exercises it.
