---
# kinora-bwo8
title: Resolver/render should materialize kinos from pruned Never-policy roots
status: completed
type: bug
priority: high
created_at: 2026-04-21T01:30:43Z
updated_at: 2026-04-21T01:50:09Z
blocked_by:
    - kinora-bayr
---

Under kinora-bayr, Never-policy roots prune owned store events from staging after archive. The current Resolver (crates/kinora/src/resolve.rs) builds identities only from staged events — so after a Never commit, resolve/render can no longer find the kinos that were archived. The content blobs still exist in the content store, and the root kinograph still lists them (id, version, kind, metadata) — those three sources together are sufficient to materialize an Identity without the staged event. 

Symptom observed: render tests in crates/kinora-cli/src/render.rs and commit tests in crates/kinora/src/commit.rs originally used policy="never" as a no-op-prune placeholder. As part of kinora-bayr I switched them to keep-last-10 to work around this. Real users who use Never intentionally would lose resolve/render access to their committed kinos.

## Fix direction

Resolver::load should also ingest root kinographs: for each (id, version) pair in any committed root blob, create or extend an Identity whose head event is reconstructed from the kinograph entry (the kino_id, version, kind, metadata fields carry the needed data, and the blob can be read on demand via ContentStore).

## Tests to add

- resolve_by_id works after Never commit with store events pruned
- render_committed_main_groups_under_main with policy "never" (the original test intent)
- integration: commit main (Never) → resolve → render all succeed end-to-end

## Plan

- [x] Write failing tests (bwo8 kinograph-ingestion tests in resolve.rs)
- [x] Extend Resolver::load with a third pass: walk roots/, read each root kinograph blob, synthesize Identity entries from (id, version, kind, metadata) when no staged event already covers (id, version)
- [x] Verify the new tests pass
- [x] Revert bayr's Never→keep-last-10 workarounds in render.rs (commit.rs workarounds are about pin/cross-root semantics; clone.rs is about assign events; neither bwo8-relevant)
- [x] Zero warnings; full workspace tests pass (502 tests: 386 kinora + 116 kinora-cli)
- [x] Code review subagent (caught one real issue: dedup set must be mutable across kinograph passes; fixed in a618bdc)

## Summary of Changes

**What:** Resolver::load now ingests committed root kinographs as a third source of Identity entries, alongside the legacy per-lineage ledger and the one-file-per-event staged layout. For any (id, version) pair in a committed root that isn't already represented, a synthetic store event is materialized from the kinograph entry's (id, version, kind, metadata). Content reads via ContentStore still work because the pinned version hash IS the content-store key.

**Why:** kinora-bayr prunes owned staged store events from Never-policy roots after archive. Without this fix, resolve/render lose sight of kinos committed under Never — the blobs exist, the pointer exists, but the identity map doesn't. bwo8 closes that gap.

**Files changed:**
- crates/kinora/src/resolve.rs — new  helper; new  error variant; inlined  (inlined to avoid a ResolveError ↔ CommitError ↔ KinographError type cycle); 5 new tests.
- crates/kinora-cli/src/render.rs — reverted kinora-bayr's  helper workaround from keep-last-10 back to Never, so  tests the policy it was originally written for.

**Commits:**
1. 6677c7b — failing tests
2. cbe3138 — implementation
3. a618bdc — review fix: dedup set must be mutable across kinograph passes (caught by review subagent; now pinned by )

**Scoping notes:** The commit.rs tests that bayr swapped Never→keep-last-10 are NOT affected by bwo8 — they test pin semantics and cross-root reassign, which happen at commit time (before resolve sees anything). The clone.rs test workaround is about assign events (not store events), which the Resolver's kinograph pass deliberately doesn't touch. Both workarounds stay.

**Tests:** 503 workspace tests pass (387 kinora + 116 kinora-cli); zero compiler warnings.
