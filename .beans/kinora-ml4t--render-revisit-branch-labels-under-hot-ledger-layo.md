---
# kinora-ml4t
title: 'Render: revisit branch labels under hot-ledger layout'
status: completed
type: task
priority: low
created_at: 2026-04-19T06:23:45Z
updated_at: 2026-04-19T08:27:59Z
parent: kinora-w7w0
---

Under the hot-ledger layout (kinora-xi21), new events live in `.kinora/hot/<ab>/<hash>.jsonl` and don't belong to any legacy per-lineage ledger file. But the render layer still groups pages under a branch label derived from the old `.kinora/ledger/<lineage>.jsonl` naming — on dogfood runs this shows up as the `7a155b58` label (the shard of RFC-0003's birth lineage) being applied to every page, including unrelated design-principle kinos.

Observed while completing kinora-ve9g. Not urgent — pages still render and read correctly — but the grouping UX is now misleading.

## Acceptance

- [x] Decide what grouping means under hot-ledger — **per-root** (see Resolution below)
- [x] Update `render` to derive labels from event metadata or drop the grouping when not meaningful
- [x] Test covers both pure-hot and mixed hot+legacy repos

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

- [x] Swap `current_branch_label` for a roots-pointer reader
- [x] Build `owners: HashMap<id, root_name>` by reading each root's entries
- [x] Kinos not in any root → `"unreferenced"` bucket
- [x] Render groups pages under `src/<root-name>/` in the mdBook
- [x] Test: pure-hot repo with no compact → all pages under `unreferenced/`
- [x] Test: single `main` root post-compact → all pages under `main/`
- [x] Test: mixed (some compacted, some not) → correct split between root name and `unreferenced/`

## Plan

### Library changes (`crates/kinora/src/render.rs`)

1. Rename `render_for_branch(&Resolver, branch)` → `render(&Resolver, labels: &HashMap<String, String>, default_label: &str)`. Every kino id the resolver knows about gets its group label from `labels`; missing ids fall back to `default_label`.
2. Rename `RenderedPage.branch` → `RenderedPage.group` (branch is misleading under hot-ledger).
3. Rename internal helpers: `group_by_branch` → `group_by_label`, `branch_index_md` → `group_index_md`.
4. Source marker: "Rendered from branch `X`" → "Rendered from `X`".
5. `SkipReason::MultipleHeads` display: drop "branch tiebreaker" wording.
6. Skip `kind == "root"` kinos entirely from render output — roots are internal bookkeeping, not user content.

### CLI changes (`crates/kinora-cli/src/render.rs`)

1. Remove `current_branch_label` (reads legacy `.kinora/HEAD`).
2. Add `build_owners_map(kin_root) -> Result<HashMap<String, String>, CliError>`:
   - List pointer files under `.kinora/roots/` (each file's name is the root name; contents are the 64-hex version hash).
   - For each pointer, load the root blob via `ContentStore::read` and parse as `RootKinograph`.
   - For each entry id, insert `(id → root_name)` into the map.
3. Call site: `let owners = build_owners_map(&kin_root)?; let book = render(&resolver, &owners, "unreferenced")?;`

### Tests

- Library: update existing tests that call `render_for_branch(..., "main")` → build a labels map covering all stored ids or use the `default_label` fallback. Keep behavioural coverage.
- Library new: `render_groups_pages_by_label_map`, `render_falls_back_to_default_label_for_unmapped_id`, `render_skips_root_kind_kinos`.
- CLI new: `build_owners_map_returns_empty_when_no_roots_dir`, `build_owners_map_maps_entries_to_root_name`, and three end-to-end render tests matching the bean's acceptance:
  - pure-hot repo with no compact → all pages under `unreferenced/`
  - single `main` root post-compact → all pages under `main/`
  - mixed (some compacted, some hot-only) → correct split

### Commit plan

1. **Tests commit**: stub the new library + CLI signatures (return empty), update every existing test to the new API, add the new tests. Tests compile; failures are assertion failures, not compile errors (per CLAUDE.md TDD rule).
2. **Implementation commit**: replace stubs with real logic (label lookup, root-kind skip, `build_owners_map`). All tests pass; zero warnings.
3. **Review commit** (if needed): fixes from subagent review.

## Summary of Changes

Rendered pages are now grouped by **owning root kinograph name** instead of the legacy "branch" label (which was meaningless under the hot-ledger layout).

**Library (`crates/kinora/src/render.rs`)**
- Renamed `render_for_branch(resolver, branch)` → `render(resolver, labels: &HashMap<String, String>, default_label: &str)`.
- Renamed `RenderedPage.branch` → `RenderedPage.group`.
- Renamed internal helpers: `group_by_branch` → `group_by_label`, `branch_index_md` → `group_index_md`.
- Source marker: "Rendered from branch `X`" → "Rendered from `X`".
- `SkipReason::MultipleHeads` display: dropped "branch tiebreaker" wording.
- Skip `kind == "root"` kinos entirely from render output — roots are internal bookkeeping, not user content.

**CLI (`crates/kinora-cli/src/render.rs`)**
- Removed `current_branch_label` (read legacy `.kinora/HEAD`).
- Added `build_owners_map(kin_root) -> Result<HashMap<String, String>, CliError>` that scans `.kinora/roots/`, loads each root blob via `ContentStore`, parses it as a `RootKinograph`, and records every entry id under its root name.
- Pointer names are sorted before iteration for deterministic multi-root insertion order (until phase 3 enforces exclusive ownership).
- NotFound roots dir → empty map (pure-hot repos render under `"unreferenced"`).
- Tmp files (`.<name>.tmp`) and non-file entries under `.kinora/roots/` are silently skipped.
- Introduced `CliError::{Store, Root}` variants for the new downstream error types.

**Tests**
- Library: `render_uses_label_from_map_when_id_is_present`, `render_falls_back_to_default_label_for_unmapped_id`, `render_skips_root_kind_kinos` (+ rename `branch_label_propagates_to_every_page` → `default_label_propagates_to_every_page_when_map_is_empty`).
- CLI: `build_owners_map_empty_when_no_roots_dir`, `build_owners_map_maps_entries_to_root_name`, `build_owners_map_ignores_tmp_and_non_file_entries`, plus three end-to-end render tests — pure-hot (`unreferenced/`), compacted `main`, and mixed.

All 231 + 54 tests pass; zero compiler warnings.

**Commits**
- `837e921` — test(render): per-root grouping API + failing tests
- `d45aa6e` — feat(render): group pages by owning root
- `19d850a` — fix(render): deterministic root ordering + tmp-file skip test
