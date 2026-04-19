---
# kinora-f0rg
title: 'Phase 3.7: cross-root integrity (external refs prevent GC drops)'
status: completed
type: task
priority: normal
created_at: 2026-04-19T10:19:10Z
updated_at: 2026-04-19T13:18:14Z
parent: kinora-hxmw
blocked_by:
    - kinora-mngq
---

If root A references (via composition) a kino owned by B, B's GC must not drop that version.

Final piece of phase 3 (kinora-hxmw). Enforces the xi21 invariant: a kino referenced by a composition kinograph (or any other kino) that is owned by a different root cannot be GC'd by its owning root. This makes composition across roots safe: you can depend on `rfcs/foo` from `main/bar` without worrying that rfcs' aggressive policy will silently break `main/bar`.

## Scope

### In scope

- [x] Pre-GC pass in `compact_root(B)`:
  - Walk every other root's content (via their last-compacted root-kinograph blobs).
  - For each composition kinograph or referencing kino found there, collect the set of (id, version) pointers that live in root B.
  - Treat those pointers as implicit pins for this compaction — they survive GC even if policy would otherwise drop them.
- [x] `compact_all` passes a shared "external references" set into each `compact_root` call so the O(N roots × N events) walk happens once per compact_all invocation, not per root.
- [x] The check considers **composition** references (via `Kinograph` content), not just any hash that happens to appear in bytes. Only kinograph-kind kinos contribute references.
- [x] When a cross-root reference saves an entry from GC, surface it in the compact CLI output so users know why something survived:
  ```
  root=rfcs  version=<sh> (new version; 2 entries retained by cross-root refs from main)
  ```
- [x] Tests:
  - Root A (`never` policy) has a kinograph that composes kino X from root B (`30d` policy, X is 40 days old).
  - Without integrity check: B's GC would drop X.
  - With integrity check: X survives B's GC; the compact output mentions the retention.
  - Removing the reference (composing a different kino) and re-compacting: X is now eligible for GC again.
  - Circular reference (A references B which references A): no infinite loop; both roots' compacts complete.

### Out of scope (deferred)

- Non-composition reference tracking (future bean if needed)
- CLI flag to force-override the integrity check

## Acceptance

- [x] All sub-points under "In scope" implemented with tests
- [x] Zero compiler warnings
- [x] Bean todo items all checked off
- [x] Summary of Changes section added at completion

## Plan

### Semantics

**External reference:** a `(target_id, target_version)` pointer originating in a composition kinograph owned by some other root. `target_version` is either the `pin` field (when explicit) or the current head version of `target_id` (when the kinograph references the head).

**Cross-root integrity rule:** during `compact_root(B)`, any entry in root B whose `(id, version)` matches an external reference from another root is treated as **implicitly pinned** — it is protected from GC and its hot event is protected from prune, even if policy would otherwise drop it.

**Scope of "external":** only references from composition kinographs (i.e. `kind="kinograph"` entries in other roots). Arbitrary hash co-occurrences don't count. Root A referencing its own kino doesn't cross-protect itself (explicit pin already covers that case).

**When references are collected:** a single snapshot of every OTHER root's last-compacted root-kinograph at the start of the compaction. `compact_all` precomputes once; a standalone `compact_root(B)` call computes its own snapshot.

**Circular references:** handled naturally — the walk iterates root entries flatly; there's no recursive traversal. A refs B refs A: A's walk finds B's reference to A and vice versa.

**Concurrency:** external-ref snapshots are frozen at compact start. If root X compacts mid-batch and produces a new version, the snapshot is still based on X's pointer-at-start. This is conservative: B may retain entries that X no longer references after its compact, but never the reverse.

**Unpinned composition references:** when a kinograph `Entry` has empty `pin`, it "references the head". We resolve that to the current head version (via `pick_head`) of the referenced id at snapshot time. If resolution fails (no events, forked), we skip — the integrity check is best-effort, not a correctness gate.

### Types to add

```rust
/// (target_id, target_version) → set of referencing-root names.
/// Built once per compact_all invocation (or per standalone compact_root).
pub struct ExternalRefs {
    by_target: BTreeMap<(String, String), BTreeSet<String>>,
}
```

`CompactResult` gains `retained_by_cross_root: BTreeMap<String, usize>` (referencing-root → count) so the CLI can render the hint.

### API shape

- `ExternalRefs::collect(kinora_root, declared_roots, self_root, events) -> Result<Self, CompactError>`: walks every declared root other than `self_root`.
- `compact_root` internally computes `ExternalRefs` when not provided. An inner helper `compact_root_with_refs` takes the precomputed snapshot; `compact_all` uses that path.
- `apply_root_entry_gc` and `prune_hot_events` grow an `implicit_pinned_versions: &BTreeSet<String>` parameter. Entries/events matching these are protected identically to explicit pins.

### CLI rendering

`render_compact_entry` extends the Ok branch to append a parenthetical `(new version; N entries retained by cross-root refs from <root>[, <root2>])` when `retained_by_cross_root` is non-empty.

### Commit sequence

1. `test(compact): f0rg cross-root integrity — failing tests`
2. `feat(compact): external refs prevent cross-root GC drops`
3. `feat(cli): render cross-root retention hint on compact output`
4. (optional) review-fix commit

## Summary of Changes

Four commits on branch `main`:

1. `48f4ae0 test(compact): f0rg cross-root integrity — failing tests`
2. `79e57eb feat(compact): external refs prevent cross-root GC drops`
3. `71a8834 feat(cli): render cross-root retention hint on compact output`
4. `df56d2e fix(compact): review fixes for f0rg — (id, version) keying + coverage gaps`

### Library (`crates/kinora/src/compact.rs`)

- **`ExternalRefs` struct**: `by_target: BTreeMap<(String, String), BTreeSet<String>>` — maps `(target_id, target_version)` to the set of source-root names whose composition kinographs reference it. Built once per `compact_all` invocation (and once per standalone `compact_root` call) by walking every declared root's last-compacted root kinograph, reading each `kind="kinograph"` entry's blob, parsing with `Kinograph::parse`, and recording each composition entry's target. Unpinned composition entries resolve to the head via `pick_head`; resolution failures are skipped (best-effort integrity).
- **Unreadable source roots skipped**: a malformed/missing root pointer or blob doesn't surface as a hard failure at collection time — the owning root's own per-root compact will raise the real error on its turn. This preserves the pre-existing `compact_all_per_root_errors_do_not_short_circuit_clean_roots` invariant.
- **`compact_root` refactor**: the public entry point now computes its own snapshot; `compact_all` computes once at batch start and passes it through to an inner helper `compact_root_with_refs`. Both paths converge — the inner helper is where the actual compaction logic lives.
- **`apply_root_entry_gc` extended**: now takes `implicit_pinned: &BTreeSet<(String, String)>` (the `(id, version)` pairs that external roots reference) and returns a `BTreeMap<String, usize>` report of `referencing-root → count` for entries that were rescued from GC. Keying on `(id, version)` pairs (not just `version` alone) prevents content-hash collisions between unrelated kinos from cross-contaminating protection.
- **`prune_hot_events` extended**: same `(id, version)` pair keying via an `is_implicit_pinned` closure; explicit `pin: true` entries still match on version alone (they're already id-scoped by virtue of living in the root's `entries`).
- **`CompactResult.retained_by_cross_root: BTreeMap<String, usize>`**: surfaces the retention report to the CLI.
- **Self-filter at query time**: `ExternalRefs::implicit_pinned_versions(self_root)` and `::referencing_roots(id, version, self_root)` drop entries whose only source is `self_root` — a root composing its own kino doesn't self-protect (explicit pin already handles that).

### CLI (`crates/kinora-cli/src/compact.rs`)

- **`render_retention_hint` helper**: formats the retention map as a trailing clause inside the status parens:
  ```
  root=inbox version=<sh> (new version; 2 entries retained by cross-root refs from main)
  root=inbox version=- (no-op; 1 entry retained by cross-root refs from main, rfcs)
  ```
  Root names render in BTreeMap sort order. The leading count is the total-retention sum — a single entry referenced by two roots contributes to both tallies, so the number reads as "total retention events". Singular/plural `entry/entries` flips based on the total.

### Tests

Five new tests total:

Library (`crates/kinora/src/compact.rs`):
- `cross_root_ref_from_a_prevents_b_gc_from_dropping_referenced_version`
- `removing_cross_root_reference_allows_subsequent_gc_drop`
- `circular_cross_root_references_do_not_loop`
- `compact_all_snapshot_taken_at_batch_start_protects_across_roots` (review-fix)
- `overlapping_refs_from_two_roots_both_count_in_retention` (review-fix)

CLI (`crates/kinora-cli/src/compact.rs`):
- `render_compact_entry_appends_retention_hint_from_single_root`
- `render_compact_entry_appends_retention_hint_from_multiple_roots_sorted`
- `render_compact_entry_retention_hint_uses_singular_for_one_entry`
- `render_compact_entry_retention_hint_attaches_to_no_op_too`

Plus one new test helper: `store_kinograph` (authors a composition kinograph blob in one call).

### Review

Fresh-eyes subagent review on `79e57eb..71a8834` flagged three real issues, all fixed in `df56d2e`:

1. Potential content-hash collision between unrelated kinos → now keyed on `(id, version)` pairs end-to-end.
2. `compact_all` had no f0rg coverage → added dedicated batch-snapshot test.
3. "Total retention vs unique entries" design wasn't locked in by a library test → added overlapping-refs test.

### Tests

301 library + 81 CLI tests pass, zero compiler warnings.
