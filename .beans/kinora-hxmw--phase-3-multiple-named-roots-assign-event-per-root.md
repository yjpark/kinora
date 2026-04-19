---
# kinora-hxmw
title: 'Phase 3: multiple named roots + assign event + per-root GC'
status: todo
type: feature
created_at: 2026-04-19T07:47:10Z
updated_at: 2026-04-19T07:47:10Z
parent: kinora-xi21
blocked_by:
    - kinora-61f9
---

Phase 3 of the xi21 architecture. Today phase 2B (kinora-61f9) produces a single flat `main` root via `kinora compact`. Phase 3 generalizes that to multiple named roots and introduces the `assign` event as the mechanism for moving a kino between roots.

## Motivation

Under xi21 §7–§8, a kino's metadata home is its leaf in its **owning** root — ownership is exclusive. Today there's only one root (`main`), so ownership is implicit. Phase 3 makes ownership explicit and manageable:

- Repos declare named roots in `config.styx` with per-root GC/prune policies (§5).
- `inbox` is auto-provisioned as the default root for unassigned kinos (§6), with an aggressive default policy to nudge triage.
- An `assign` event records "kino X should live in root Y from now on"; compaction consumes assign events and moves the entry between roots' next versions (§19 in the four-concept model).

## Scope

### In scope

- [ ] Config: declare named roots in `config.styx` with per-root policies (e.g. `never`, `30d`, `keep-last-N`)
- [ ] Auto-provision `inbox` as default root if not declared
- [ ] Event schema: generalize hot-ledger events to include non-store event kinds (today every hot event is a store event)
- [ ] `assign` event type: `{ kind: "assign", kino_id, target_root, author, ts, provenance }`
- [ ] `kinora assign <kino-or-name> <root>` CLI command — writes an assign event to the hot ledger
- [ ] Compaction consumes assign events: when compacting root Y, include kinos with a pending `assign → Y` even if they're in root X; remove from root X in that same compaction pass
- [ ] `kinora compact` without `--root` compacts **all** declared roots (preserving phase-2 `--root main` default for single-root invocations)
- [ ] GC/prune: each root's compaction prunes hot events older than its policy, plus drops entries whose content versions are older than policy and not pinned
- [ ] Pin support: `pin: true` on a root entry exempts it from GC
- [ ] Cross-root integrity: if root A references (via composition) a kino owned by root B, B's GC must not drop that version; enforced at compact time
- [ ] Tests cover: assign event round-trip, multi-root compact determinism, inbox auto-provision, GC drops aged entries, pin exempts, cross-root pin enforcement

### Out of scope (deferred)

- Merkle sub-kinographs inside roots (phase 4)
- Moving between existing roots during a single compaction without an explicit assign (not a feature)
- Config hot-reload — a config change takes effect on next compact

## Open design questions

1. **Config shape.** Styx-native sugar for root declarations, or a generic `roots {}` block? Suggest: `root inbox { policy "30d" } root rfcs { policy "never" }` — reads well, parses with existing styx.
2. **Assign precedence when a kino has multiple pending assigns.** Last-writer-wins by `ts`? Or reject at compact time as ambiguous? Suggest last-writer-wins — aligns with the "event log as source of truth" model; ties broken by event hash order.
3. **Do we emit an assign event at birth, or is birth implicit-assign-to-inbox?** Suggest implicit: no event needed; kinos without a prior assign default to `inbox` at compact time. Keeps the hot ledger lean.
4. **What happens when a kino is assigned to a root that doesn't exist?** Suggest: compact fails loudly with `UnknownRoot { name }`. Alternative: route to `inbox` — but that hides typos.
5. **Does `kinora compact` without `--root` fail or succeed when one declared root has no changes?** Suggest: per-root no-op detection (each root independent), CLI prints one line per root with `(no-op)` or `(new version)`.

## Acceptance

- [ ] All sub-points under "In scope" implemented with tests
- [ ] `kinora compact` continues to work with `--root main` (phase-2 compatibility preserved)
- [ ] Rendering (kinora-ml4t) can group by owning root unambiguously — every kino has exactly one owning root post-phase-3
- [ ] Zero compiler warnings
- [ ] Bean todo items all checked off
- [ ] Summary of Changes section added at completion

## Provenance

Broken out of `kinora-xi21` (phase 3) on 2026-04-19 following the per-root render decision for kinora-ml4t.
