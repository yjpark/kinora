---
# kinora-ml4t
title: 'Render: revisit branch labels under hot-ledger layout'
status: todo
type: task
priority: low
created_at: 2026-04-19T06:23:45Z
updated_at: 2026-04-19T07:47:49Z
parent: kinora-w7w0
---

Under the hot-ledger layout (kinora-xi21), new events live in `.kinora/hot/<ab>/<hash>.jsonl` and don't belong to any legacy per-lineage ledger file. But the render layer still groups pages under a branch label derived from the old `.kinora/ledger/<lineage>.jsonl` naming — on dogfood runs this shows up as the `7a155b58` label (the shard of RFC-0003's birth lineage) being applied to every page, including unrelated design-principle kinos.

Observed while completing kinora-ve9g. Not urgent — pages still render and read correctly — but the grouping UX is now misleading.

## Acceptance

- [x] Decide what grouping means under hot-ledger — **per-root** (see Resolution below)
- [ ] Update `render` to derive labels from event metadata or drop the grouping when not meaningful
- [ ] Test covers both pure-hot and mixed hot+legacy repos

## Notes

Scope is UX/render-layer only; no data model changes expected.

## Blocked — needs design decision

The first acceptance item is itself the open question: what does 'branch' mean under the hot-ledger layout? The bean lists three candidates (per-author, per-worktree, none), and the answer shapes the rest of the work (test matrix, label derivation). An autonomous pass could pick one, but the choice is UX-visible and hard to reverse later, so it's better to surface than guess.

### Current behaviour (for reference when deciding)

- `render::render_for_branch` takes a `branch: impl Into<String>` and stamps every page with it for SUMMARY.md grouping.
- `kinora-cli/src/render.rs :: current_branch_label` derives the label from `.kinora/HEAD` (legacy per-lineage layout) and falls back to `"main"`. Under hot-ledger writes (kinora-mjvb), HEAD is never populated by new events — so on a pure-hot repo the label is always `"main"`, and on a mixed repo it's whatever lineage happened to be active at the last legacy write.

### Options (brief)

1. **none** — drop the grouping; rendered pages live flat under `src/`. Simplest; matches the data model (hot-ledger has no branch concept).
2. **per-author** — group by `event.author` of the head event. Meaningful grouping; but authorship is a social signal, not a topology one, and dogfood repos often have one author.
3. **per-root** — (not in the bean's original list, but worth considering post-kinora-61f9) group by owning root kinograph. Aligns with the xi21 curated-view model; most natural grouping now that roots exist.
4. **per-worktree** — probably not meaningful since pages are rendered from resolver state, not worktree state.

### Suggested next step

Pick one of (1) or (3) and promote this bean back to todo with the decision recorded. (2) and (4) feel weaker. Leaning (3) because roots are now the native grouping primitive under xi21; but (1) is the minimum-change path and would unblock the render UX fix today.

## Resolution (2026-04-19)

Group rendered pages by **owning root**. Under xi21 §7–§8 ownership is exclusive — every kino has exactly one owning root once phase 3 (kinora-hxmw) lands. Today (phase 2) there is only a single root (`main`), so grouping is trivially one bucket; the structure is future-proof for multi-root without render-layer rework.

### Sub-cases

- **Kino's owning root exists**: page goes under `src/<root-name>/` in the rendered mdBook. Labelled by root name (`main`, `rfcs`, `inbox`, …) — not by an opaque shorthash.
- **Kino is not yet compacted into any root** (pre-first-compact, or post-hot-write pre-next-compact): group under `unreferenced/`. Phase 3 makes this bucket near-empty by auto-assigning to `inbox`; phase 2 users will see it populated until they run `kinora compact`.
- **Multiple roots (phase 2)**: can't happen — only `main` exists. No code needed for this case today beyond reading the one pointer file.
- **Multiple roots (phase 3+)**: each kino is owned by exactly one, so each page appears in exactly one group. Composition kinographs (which are not roots) don't affect grouping.

### Implementation sketch

1. Replace `current_branch_label` in `kinora-cli/src/render.rs` with a loader that reads all root pointers under `.kinora/roots/` and, for each, reads the root blob to get its entry ids.
2. Replace `render_for_branch` signature with something like `render_for_roots(resolver, owners: HashMap<id, root_name>)` — or keep the per-page-stamp shape but derive the stamp from owning root instead of a single global label.
3. `write_book` already groups by `page.branch`; the directory naming becomes root name.
4. Tests: pure-hot repo with no compact (everything in `unreferenced/`), single `main` root post-compact, and (deferred to phase 3) multi-root with assign events.

### Scope note

This bean can land in phase 2 — it doesn't block on kinora-hxmw. The `unreferenced/` bucket is how pre-compact kinos are handled today; phase 3 will shrink that bucket by auto-assigning to `inbox` but won't change the render code paths.

- [ ] Swap `current_branch_label` for a roots-pointer reader
- [ ] Build `owners: HashMap<id, root_name>` by reading each root's entries
- [ ] Kinos not in any root → `"unreferenced"` bucket
- [ ] Render groups pages under `src/<root-name>/` in the mdBook
- [ ] Test: pure-hot repo with no compact → all pages under `unreferenced/`
- [ ] Test: single `main` root post-compact → all pages under `main/`
- [ ] Test: mixed (some compacted, some not) → correct split between root name and `unreferenced/`
