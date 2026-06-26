---
# kinora-9as9
title: Re-versioning a committed kino forks across roots (synthesis drops lineage)
status: completed
type: bug
priority: high
created_at: 2026-06-26T00:47:51Z
updated_at: 2026-06-26T01:00:00Z
---

Revising a kino after it has been committed produces a fork: 'resolve' reports MultipleHeads and 'commit' becomes a no-op because no single head can be picked.

## Symptom

`kinora store markdown --id X --parents H <new>` followed by `kinora commit`
produces a fork: `kinora resolve X` reports "has 2 heads" and `kinora commit`
leaves that kino's root a no-op (cannot pick a single head).

## Root cause (confirmed by reproduction)

`ingest_root_kinographs` in `crates/kinora/src/resolve.rs` synthesizes an Event
from each committed root-kinograph entry with **empty parents** (resolve.rs:306,
`vec![]`). The code comment (resolve.rs:128-131) claims this is safe because a
later staged v2 with `parents=[v1.hash]` still demotes the synthesized v1 — but
that only holds while v2 is still *staged*. Once v2 is itself committed and
pruned (Never policy), it too is synthesized with empty parents.

The trigger is **cross-root**: a bare `store --id X --parents H` carries no live
assign, so at commit it routes to the default `inbox` while the original stays
in its root (e.g. `main`). After both roots commit+prune, the resolver
synthesizes v1 (from main) and v2 (from inbox), **both with empty parents** —
neither references the other, so both are heads → `MultipleHeads`.

Same-root revision (re-asserting the assign each time) does NOT fork, because
the root entry for the kino is replaced and only one version is synthesized.

## Reproduction (as failing tests, in resolve.rs)

- `revising_a_committed_kino_without_reassign_does_not_fork` — FAILS (forks)
- `revising_a_committed_kino_in_same_root_does_not_fork` — passes (boundary)

## Fix

Preserve lineage through the root kinograph. `RootEntry` carries no per-entry
parents today; add an optional `parents` field populated from the head event's
parents at `build_root` time (when the head is still a staged event with real
parents). `ingest_root_kinographs` then synthesizes with those parents instead
of `vec![]`, so an ancestor version is demoted even after it has been pruned.

## Plan

- [x] Add failing tests reproducing the cross-root fork
- [x] Add `parents: Vec<String>` to `RootEntry` (root.rs) + styxl read/write, backward-compatible (absent = empty)
- [x] `build_root` (commit.rs) populates entry parents from the head event's parents
- [x] `ingest_root_kinographs` (resolve.rs) synthesizes with entry parents
- [x] Verify both new tests pass + full workspace suite green, zero warnings
- [ ] Code review pass

## Code Review

Fresh-eyes subagent review of the test + fix commits. Core fix verified
correct (version-vs-event hash usage consistent; prior-root merge preserves
parents; no-op detection unaffected). Addressed:

- [x] Issue 1 (medium): root pointer iteration was filesystem-order-dependent;
      now sorts root names and UNIONS parents across roots on (id,version)
      collision, so head resolution is deterministic and order-independent.
- [x] Issue 2 (minor): fixed stale doc comment that referenced nonexistent
      `resolver_chains_*` tests.
- [x] Issue 3 (minor): `validate_entry` now validates each `parents[]` hash.
- [x] Issue 4 (coverage): added a 3-version chain split across three roots
      test, a parent-union test, and a parents-hash-validation test.

## Summary of Changes

Root cause: `ingest_root_kinographs` synthesized events from committed root
kinograph entries with empty parents, discarding lineage. The safety claim in
the old comment ("a staged v2 still demotes the synthesized v1") only held
while v2 was still staged; once v2 was itself committed and pruned, both
versions were synthesized parent-less and neither demoted the other. This
surfaced whenever a kino's versions ended up in different roots — the common
case, since a bare `store --id X --parents H` carries no live assign and routes
to inbox while the original stays in its root.

Fix: carry lineage on the durable record.
- `RootEntry` gains an optional `parents: Vec<String>` field (backward-compatible
  via `#[facet(default)]`; absent on legacy blobs = empty). Validated as hashes.
- `build_root` populates it from the head event's parents (captured at commit
  time, while the head is still staged with authentic parents).
- `ingest_root_kinographs` synthesizes with those parents, processes root
  pointers in sorted order, and unions parents across roots on (id,version)
  collision for deterministic, order-independent head resolution.

Tests: cross-root 2-version (was the failing repro), same-root revision
(boundary), 3-version chain split across three roots, parent union across
roots, RootEntry parents roundtrip, legacy-entry default, invalid-parent-hash
rejection. Full workspace suite green (398 + 115 + 72 + 26), zero warnings.

Files: crates/kinora/src/{root.rs,commit.rs,resolve.rs}.
