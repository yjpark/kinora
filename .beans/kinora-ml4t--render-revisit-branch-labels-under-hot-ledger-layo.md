---
# kinora-ml4t
title: 'Render: revisit branch labels under hot-ledger layout'
status: draft
type: task
priority: low
created_at: 2026-04-19T06:23:45Z
updated_at: 2026-04-19T07:21:06Z
parent: kinora-w7w0
---

Under the hot-ledger layout (kinora-xi21), new events live in `.kinora/hot/<ab>/<hash>.jsonl` and don't belong to any legacy per-lineage ledger file. But the render layer still groups pages under a branch label derived from the old `.kinora/ledger/<lineage>.jsonl` naming — on dogfood runs this shows up as the `7a155b58` label (the shard of RFC-0003's birth lineage) being applied to every page, including unrelated design-principle kinos.

Observed while completing kinora-ve9g. Not urgent — pages still render and read correctly — but the grouping UX is now misleading.

## Acceptance

- [ ] Decide what 'branch' means under hot-ledger (per-author? per-worktree? none?)
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
