---
# kinora-ohwb
title: 'Render: multi-branch + worktree enumeration'
status: completed
type: feature
priority: normal
created_at: 2026-04-18T16:42:01Z
updated_at: 2026-04-20T14:34:29Z
parent: kinora-w7w0
blocked_by:
    - kinora-9nom
---

Extend the render command to union ledger files across all local branches and worktrees, rendering each as a top-level SUMMARY.md section.



## Context

Split out of kinora-9nom. The MVP render delivered by kinora-9nom targets the current branch only. The library layers there were designed to accept `Vec<(branch, Resolver)>` so wiring multi-source input is straightforward.

## Scope

- Enumerate local branches via `gix::Repository::references()` filtered to `refs/heads/*`
- For each branch: peel to tree, look up `.kinora/ledger/` subtree, read each `.jsonl` blob, parse into events, build a per-branch Resolver
- Enumerate worktrees via `gix::Repository::worktrees()` and treat each as its own "branch label" using the worktree's checked-out ref name
- Wire the list through the existing render library; one top-level SUMMARY.md section per branch/worktree
- Source marker on each page cites its originating branch

## Acceptance

- [x] `gix` walk reads `.kinora/ledger/*.jsonl` at a specific commit
- [x] Events from all branches surface in `Resolver` structure (per-branch, not unioned — each branch renders independently)
- [x] Worktrees enumerated; HEAD commit of each worktree resolved
- [x] SUMMARY.md has one top-level group per branch/worktree
- [x] A kino that exists only on branch B appears only under branch B's section
- [x] Kinos present on multiple branches appear once per branch (duplicated pages, clearly labelled)

## Draft notes

Still draft — design needs confirmation:
- Should identical kinos across branches (same `id`) dedupe, or render once per branch? Current plan: once per branch with a source marker (simpler; matches git's branch isolation).
- How should `kino://<id>/` cross-links resolve when the target kino lives on a different branch? Options: (a) pick current-branch target preferentially with fallback to any branch; (b) always link to the current branch's section and warn if missing; (c) emit a disambiguated URL per branch.

## Resolved: design questions

**Q1 — dedupe vs per-branch rendering:** render per-branch with source marker. One page per (branch, kino) pair; same-id kinos on multiple branches get duplicated pages. Matches git's branch-as-independent-timeline model; content-addressed kinos can legitimately have different versions on different branches, so dedupe-to-one would be lossy. Duplication cost is negligible.

**Q2 — cross-branch `kino://<id>/` resolution:** within-branch only. Resolve against the current branch's Resolver. If the target doesn't exist on the current branch, emit the reference as text with a visible warning marker — no dead link, no cross-branch fallback. Users who want working links merge the target branch. Rationale: cross-branch references are an anti-pattern git already handles via merge; don't build machinery for a case with a built-in solution.


## Plan

Library-first, test-driven.

**1. New module `crates/kinora/src/git_state.rs`** with three functions:
- `extract_subtree(repo, commit_oid, subtree_path, dst)` — walk a commit's tree, write each blob under `<subtree_path>/` to `dst/` preserving structure. Skip symlinks/gitlinks. Error if subtree absent.
- `list_local_branches(repo) -> Vec<(name, tip_oid)>` — enumerate `refs/heads/*`, peel to commit id.
- `list_worktrees(repo) -> Vec<WorktreeInfo { label, head_commit, ref_name }>` — enumerate linked worktrees via `repo.worktrees()`. Main worktree excluded (matches `git worktree list` semantics — main already surfaced via branch enumeration).

**2. CLI `crates/kinora-cli/src/render.rs`** — change from single-resolver render to multi-source:
- Open the git repo at `repo_root`. If gix open fails (non-git or bare), fall back to current-behavior: render `.kinora/` from working dir with group label "working-copy".
- Enumerate branches + worktrees. Dedupe: if a worktree's `ref_name` matches a branch already in the set, skip (avoid duplicate renders of same tree).
- For each surviving source: extract `.kinora/` subtree to a tempdir, call `Resolver::load` + `build_owners_map`, call `render(&resolver, &HashMap::new(), label)` — use empty labels map + branch-name as default label, so every page gets group=branch_name. The root-based ("main"/"unreferenced") grouping of kinora-9nom is subsumed by branch grouping. Root owner info is preserved in each page's source marker footer via a new `root` field on `RenderedPage` if worth it, or simply dropped — the current marker already shows the group (now the branch).
- Merge all per-source Books into one combined Book before `write_book`.

**3. Integration tests** against fixture git repos (use `gix::init` + git commands via subprocess or gix's commit API). Seed two branches with different `.kinora/` states, verify rendered pages land under the right branch sections.

**Deferred to follow-up if time runs short:**
- Worktree enumeration (main-branch-only MVP is most of the value; worktrees are rarer)
- Cross-branch `kino://<id>/` resolution — current behavior is per-branch resolver, which by acceptance Q2 is the desired behavior

## Summary of Changes

**Commits:**
- `1ae4128` feat(git_state): new library module with `extract_subtree`, `list_local_branches`, `list_worktrees`
- `e5e0ddb` feat(render): wire git_state into multi-branch render
- `1a3360f` test(render): cover SubtreeAbsent-per-branch and detached-HEAD worktree (review fixes)

**What changed:**
- New `kinora::git_state` module reads committed `.kinora/` state per branch/worktree via gix 0.81 — no working-copy dependency.
- `kinora render` now enumerates local branches (`refs/heads/*`) + linked worktrees and emits one top-level group per source in SUMMARY.md. Kinos committed only on branch B appear only under B; kinos on multiple branches render once per branch with a branch-cited source marker.
- Sources are deduped by ref name — a worktree checked out on an existing branch doesn't double-render. Detached-HEAD worktrees keep their own `worktree-*` label.
- Pre-kinora branches (no `.kinora/` in that commit's tree) are silently skipped per-branch via `ExtractError::SubtreeAbsent` rather than aborting the whole render.
- Pre-first-commit fallback: if no branch has committed `.kinora/`, render reads from the working copy with the legacy root-based grouping, preserving backward compatibility with all existing CLI tests.
- `RenderReport.source_count` reports contributing sources (0 = fallback); CLI success line prints it.

**Design decisions realized:**
- Q1: Per-branch rendering with duplicated pages (no cross-branch dedupe) — matches git's branch-as-independent-timeline model.
- Q2: `kino://<id>/` resolves within-branch only — each branch has an independent Resolver.

**Deliberate deviation from plan:** plan suggested labeling the fallback path "working-copy"; implementation kept the legacy "unreferenced" (+ "<root>" for committed roots) grouping so existing CLI tests (`render_pure_staged_repo_groups_under_unreferenced` etc.) pass without modification. Future polish can unify on a single label.

**Known limitations (potential follow-ups):**
- `gix::open(repo_root)` doesn't walk up — if kinora lives in a subdir of a larger git repo, the render silently falls back. Worth a future bean.
- Worktree labels use the proxy id (directory basename). Two linked worktrees with the same basename would merge groups in `write_book`. Edge case.

**Test coverage:** 11 render-focused tests in `kinora-cli` (4 new git-path + 3 review-driven) + 8 in `kinora::git_state`. All 489 workspace tests pass, zero warnings.
