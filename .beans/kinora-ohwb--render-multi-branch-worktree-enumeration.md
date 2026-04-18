---
# kinora-ohwb
title: 'Render: multi-branch + worktree enumeration'
status: draft
type: feature
priority: normal
created_at: 2026-04-18T16:42:01Z
updated_at: 2026-04-18T16:42:14Z
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

- [ ] `gix` walk reads `.kinora/ledger/*.jsonl` at a specific commit
- [ ] Events from all branches surface in `Resolver` structure (per-branch, not unioned — each branch renders independently)
- [ ] Worktrees enumerated; HEAD commit of each worktree resolved
- [ ] SUMMARY.md has one top-level group per branch/worktree
- [ ] A kino that exists only on branch B appears only under branch B's section
- [ ] Kinos present on multiple branches appear once per branch (duplicated pages, clearly labelled)

## Draft notes

Still draft — design needs confirmation:
- Should identical kinos across branches (same `id`) dedupe, or render once per branch? Current plan: once per branch with a source marker (simpler; matches git's branch isolation).
- How should `kino://<id>/` cross-links resolve when the target kino lives on a different branch? Options: (a) pick current-branch target preferentially with fallback to any branch; (b) always link to the current branch's section and warn if missing; (c) emit a disambiguated URL per branch.
