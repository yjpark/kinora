---
# kinora-bwo8
title: Resolver/render should materialize kinos from pruned Never-policy roots
status: in-progress
type: bug
priority: high
created_at: 2026-04-21T01:30:43Z
updated_at: 2026-04-21T01:45:26Z
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
- [ ] Code review subagent
