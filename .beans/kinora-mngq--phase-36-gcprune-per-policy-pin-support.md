---
# kinora-mngq
title: 'Phase 3.6: GC/prune per policy + pin support'
status: in-progress
type: task
priority: normal
created_at: 2026-04-19T10:18:49Z
updated_at: 2026-04-19T12:16:55Z
parent: kinora-hxmw
blocked_by:
    - kinora-c48l
    - kinora-l79b
---

Each root compaction prunes hot events and old entries per its declared policy; pin exempts.

Sixth piece of phase 3 (kinora-hxmw). Enforces the per-root retention policies declared in config (hxmw-1) during compaction, and adds the `pin: true` escape hatch for entries that must survive GC regardless of age or depth.

## Scope

### In scope

- [ ] Policy evaluation during `compact_root`:
  - `RootPolicy::Never` → no entry is pruned, no hot event is dropped.
  - `RootPolicy::MaxAge(duration)` → drop entries whose content version `ts` is older than `now() - duration`; prune hot events older than `now() - duration` from `.kinora/hot/`.
  - `RootPolicy::KeepLastN(n)` → keep only the N most recent content versions per kino id (by `ts`); older versions are candidates for drop.
- [ ] `pin: true` on a `RootEntry` exempts that entry (and the content version it names) from all GC. Pin is set via direct root-kinograph editing today; a CLI to toggle pin is deferred.
- [ ] Hot-ledger pruning: once the event is older than the root's policy AND the compaction that made it permanent has happened, the event file can be deleted. Implementation must not race with concurrent reads — use the existing atomic write discipline.
- [ ] Multi-version preservation: if a pin points at version N, versions N-1 and later still live in the store (since content is immutable and dedup'd). Policy only controls what the root kinograph names, not what the content store holds — document this distinction.
- [ ] Tests:
  - `Never` drops nothing even when entries are years old.
  - `MaxAge("7d")` drops an 8-day-old unpinned entry but keeps a 6-day-old entry.
  - `MaxAge("7d")` + pin → entry survives regardless of age.
  - `KeepLastN(3)` with 5 versions keeps exactly the 3 newest by ts.
  - `KeepLastN(3)` + pin on version 1 → that version survives in addition to the 3 newest (pin is non-exclusive with N-window).
  - Hot-ledger events older than policy are removed from disk after compact; fresh events untouched.

### Out of scope (deferred)

- Cross-root integrity (hxmw-7)
- CLI for toggling pin (future bean)

## Acceptance

- [ ] All sub-points under "In scope" implemented with tests
- [ ] Zero compiler warnings
- [ ] Bean todo items all checked off
- [ ] Summary of Changes section added at completion

## Plan

### Semantics (explicit interpretation)

**Root-entry GC (affects the root kinograph):**
- `Never` → no entry is dropped
- `MaxAge(d)` → drop entries whose `version` event `ts` is older than `now - d`, unless `pin: true`
- `KeepLastN(n)` → does not affect root entries (the root kinograph has at most one entry per kino by `pick_head`; KeepLastN acts on the hot ledger, not the root view)

**Hot-ledger pruning (affects `.kinora/hot/`):**
- After a successful `compact_root(Y)`, prune hot events whose owning root is `Y`. Ownership is the routing decision: store events routed to Y, plus all assign events with `target_root == Y` (live or superseded).
- `Never` → no hot event dropped
- `MaxAge(d)` → drop hot events with `ts < now - d`, unless pinned
- `KeepLastN(n)` → for each kino id, keep the N newest store events by `ts`; older store events are candidates to drop. Assign events have no N-window (policy only prunes by MaxAge).

**Pin semantics:**
- `pin: true` on a `RootEntry` protects that entry *and* its `version` field from GC.
- The HOT event whose hash matches a pinned entry's `version` also survives, regardless of `MaxAge` / `KeepLastN`.
- Pin does not propagate to other events of the same kino id — only the specific pinned version is protected.

**Pin propagation across rebuilds:**
- `build_root` inherits pinned entries verbatim from the prior root kinograph for kinos still owned by this root. This preserves hand-edits and the specific `version` referenced by a pinned entry.
- Unpinned entries are rebuilt from `pick_head` as before.
- If a kino is reassigned away from root Y, its pinned entry in Y is dropped on the next compact (ownership wins over pin for routing).

**Timestamp reference:** GC uses `params.ts` (the compact run's own timestamp) as `now`. Tests can exercise boundaries by passing explicit past `ts` values to `store_kino` and a known `params.ts`.

**Duration parsing:** `RootPolicy::max_age_seconds()` returns `Some(i64)` for `MaxAge("<N><s|m|h|d|w|y>")` and `None` otherwise. `y = 365d` (calendar-agnostic).

**Cross-root integrity (deferred to f0rg):** this bean does not check whether a kino's version is referenced from another root's composition. f0rg will add that check before the drop.

### Commit sequence

1. `test(compact): mngq GC/prune/pin — failing tests`
2. `feat(compact): policy-driven GC + hot-ledger pruning + pin propagation`
3. (optional) review-fix commit
