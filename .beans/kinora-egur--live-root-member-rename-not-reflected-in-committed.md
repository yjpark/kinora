---
# kinora-egur
title: Live root-member rename not reflected in committed root kinograph
status: completed
type: bug
priority: normal
created_at: 2026-06-26T00:48:23Z
updated_at: 2026-06-26T04:14:31Z
---

A metadata-only rename (store same content with --id, no --parents) updates the ledger head but does NOT bump the root version, so the committed root kinograph keeps the old member name. Consumers must read the live head name from the ledger. The committed root should reflect the live name, or this should be documented as intended with a render-side guidance.

## Summary of Changes

Root cause: a re-store of an already-committed kino with no fresh assign
(a metadata-only rename, or any bare revision) routed to the default `inbox`
instead of the root it already lived in. The original root then kept its stale
entry (old name) via prior-root merge, and the kino effectively split across
roots.

Fix — routing inheritance (kinora-egur):
- `route_kino` (commit.rs): an unassigned kino now inherits the root that
  currently owns it (from a kino-id -> root map built from prior committed
  roots), falling back to `inbox` only for brand-new kinos. Explicit live
  assigns still win.
- `collect_root_ownership` builds the ownership snapshot once per batch
  (mirrors the `refs` snapshot); threaded through `commit_all`, `commit_root`,
  `commit_root_with_refs`, and `build_root`.
- Critically, the archive (`collect_owned_staged_events`) and prune
  (`prune_staged_events`) paths now use the SAME `route_kino` routing — a
  prior inconsistency (they defaulted to `inbox`) would otherwise prune events
  a root didn't actually commit, losing lineage.

Scope/semantics decision: only revisions of *already-owned* kinos inherit;
brand-new unassigned kinos still default to `inbox`. This is conservative,
matches user expectation ("a revision stays where it was"), and unifies with
the earlier fork fix (kinora-9as9) — a bare revision no longer splits across
roots at all.

Tests: metadata rename updates the committed root (not inbox); unassigned
revision inherits prior root while a brand-new kino still defaults to inbox.
Updated `revising_a_committed_kino_chain_split_across_roots_does_not_fork` to
force the cross-root split via explicit assigns (bare stores now inherit).
Full workspace suite green (403+122+72+26), zero warnings.
