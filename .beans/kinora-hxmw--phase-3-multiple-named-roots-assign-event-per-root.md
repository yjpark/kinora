---
# kinora-hxmw
title: 'Phase 3: multiple named roots + assign event + per-root GC'
status: todo
type: feature
priority: normal
created_at: 2026-04-19T07:47:10Z
updated_at: 2026-04-19T08:10:12Z
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

- [ ] Config: declare named roots under a `roots {}` block in `config.styx` with per-root policies (e.g. `never`, `30d`, `keep-last-N`) — see D1
- [ ] Auto-provision `inbox` as default root if not declared
- [ ] Event schema: generalize hot-ledger events to include non-store event kinds (today every hot event is a store event)
- [ ] `assign` event type: `{ kind: "assign", kino_id, target_root, supersedes: Vec<String>, author, ts, provenance }` — `supersedes` list may be empty — see D2
- [ ] `kinora assign <kino-or-name> <root> [--resolves <hashes,...>]` CLI command — writes an assign event to the hot ledger; `--resolves` populates `supersedes`
- [ ] `kinora store --root <name>` flag: when set to anything other than the implicit inbox default, writes an explicit birth-assign event alongside the store event (atomic pair) — see D3
- [ ] Compaction consumes live assign events (where live = not superseded): when compacting root Y, include kinos with a live `assign → Y`; remove from root X in that same compaction pass
- [ ] `kinora compact` always compacts **all** declared roots; no `--root` flag; per-root errors do not block clean roots — see D5
- [ ] `AmbiguousAssign` error when a kino has ≥2 live pending assigns — see D2
- [ ] `UnknownRoot` error when an assign references a root not in `config.styx` — see D4
- [ ] Library API: split phase-2B's `compact` into `compact_root` (per-root, testable) and `compact_all` (batch, CLI-facing) — see D5
- [ ] GC/prune: each root's compaction prunes hot events older than its policy, plus drops entries whose content versions are older than policy and not pinned
- [ ] Pin support: `pin: true` on a root entry exempts it from GC
- [ ] Cross-root integrity: if root A references (via composition) a kino owned by root B, B's GC must not drop that version; enforced at compact time
- [ ] Tests cover: assign event round-trip, `supersedes` resolution, ambiguous-assign error, unknown-root error, multi-root compact determinism, per-root error isolation, inbox auto-provision, birth-with-`--root` atomic pair, GC drops aged entries, pin exempts, cross-root pin enforcement

### Out of scope (deferred)

- Merkle sub-kinographs inside roots (phase 4)
- Moving between existing roots during a single compaction without an explicit assign (not a feature)
- Config hot-reload — a config change takes effect on next compact

## Resolved design decisions (2026-04-19)

### D1 — Config shape: explicit `roots {}` container block

`config.styx` declares roots in a named container, each root name a unique key mapping to its policy block. Maps cleanly to `HashMap<String, RootPolicy>` at parse time.

```
repo_url "https://..."

roots {
  inbox   { policy "30d" }
  rfcs    { policy "never" }
  designs { policy "keep-last-10" }
}
```

Rationale: conventional config shape; keeps names as map keys so duplicates are a styx-level error automatically; uses the nested-block shape styx already supports.

### D2 — Multiple pending assigns: reject as ambiguous, with `supersedes` resolution

When a kino has ≥2 pending (post-last-compact) live `assign` events, compact fails with `AmbiguousAssign { kino_id, candidates: [...] }` listing each competing assign event hash, destination, author, and ts.

**Resolution primitive**: every `assign` event carries an optional `supersedes: Vec<String>` field (list of earlier event hashes this one invalidates). Compact treats an assign as "live" iff no *other* live assign lists it in its `supersedes`. Resolution is a single new assign whose `supersedes` points at the ones it invalidates — no new event kind needed.

**CLI**:
```
$ kinora compact
error: ambiguous assigns for kino aaaa…
  - assign → rfcs    (event abc1…, yj, 2026-04-19T10:00:00Z)
  - assign → designs (event def2…, yj, 2026-04-19T11:00:00Z)
to resolve: kinora assign aaaa… <root> --resolves abc1…,def2…
```

Rationale: conflicts should fail loud, not silently resolve. Same mechanism (`supersedes`-style resolution events) will generalize to future metadata-conflict resolution, so designing the pattern now pays forward.

### D3 — Birth into a non-default root: explicit assign only when non-default (Option C)

- `kinora store` without `--root` → writes only the birth event. Compaction routes unassigned kinos to `inbox` implicitly. Hot ledger stays lean for the normal case.
- `kinora store --root rfcs` → writes birth event + an explicit `assign → rfcs` event as a pair. Atomic pair: if either fails, both back out.

A birth-assign event is indistinguishable from a later assign; no special event kind. Keeps the assign-resolution machinery (D2) uniform across birth-time and post-birth moves.

### D4 — Assign to a nonexistent root: fail loudly (Option A)

Compact errors with `UnknownRoot { name, event_hash }` listing the offending assign event. User resolves by writing a new assign to a valid root with `--resolves <offending-hash>`. Consistent with D2's "fail loud, explicit resolution" posture. Auto-create is rejected — declaring a root is an intentional config change, not a side effect of compact.

### D5 — `kinora compact` compacts all declared roots; per-root errors don't block clean roots (Option B scoping)

- `kinora compact` takes no `--root` flag. It always evaluates every declared root.
- Each root is independent: a clean root advances even if another root errors mid-batch.
- CLI output: one line per root, regardless of outcome.

```
$ kinora compact
root=main  version=<sh> (new version)
root=rfcs  ERROR: ambiguous assigns for kino aaaa… (see details)
root=inbox version=<sh> (no-op)
exit 1
```

Exit code is 0 iff every root succeeded (new version or no-op). Any root erroring → exit 1, but clean roots still advance.

**Library split**: `compact_root(kin_root, name, params) -> Result<CompactResult, CompactError>` (per-root, testable) and `compact_all(kin_root, params) -> Vec<(String, Result<CompactResult, CompactError>)>` (batch, CLI-facing). Phase-2B's current `compact` function is renamed to `compact_root`.

## Acceptance

- [ ] All sub-points under "In scope" implemented with tests
- [ ] Phase-2B's `compact_root` library fn kept intact (renamed from `compact`) so the library API is backward compatible; the `--root` CLI flag is retired in favour of always-all (see D5)
- [ ] Rendering (kinora-ml4t) can group by owning root unambiguously — every kino has exactly one owning root post-phase-3
- [ ] Zero compiler warnings
- [ ] Bean todo items all checked off
- [ ] Summary of Changes section added at completion

## Provenance

Broken out of `kinora-xi21` (phase 3) on 2026-04-19 following the per-root render decision for kinora-ml4t.
