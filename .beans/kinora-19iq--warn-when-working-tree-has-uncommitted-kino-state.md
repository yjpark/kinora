---
# kinora-19iq
title: Warn when working tree has uncommitted kino state at render time
status: completed
type: task
priority: normal
created_at: 2026-06-26T00:48:24Z
updated_at: 2026-06-26T04:00:11Z
---

render reads git-committed .kinora, not the working tree. Workflow gotcha: the loop must be store -> kinora commit -> git commit -> render. Add a kinora-side warning when the working tree has uncommitted kino state (staged events not yet git-committed) so the stale-render footgun is visible.

## Summary of Changes

`kinora render` now warns when the working-tree `.kinora/` differs from the
version committed at the current branch HEAD — surfacing the
store -> `kinora commit` -> `git commit` -> render footgun (render reads
committed git state, so un-`git commit`-ed kino activity is silently absent).

- `crates/kinora-cli/src/render.rs`:
  - `RenderReport.uncommitted_kino_state: bool`.
  - `working_tree_has_uncommitted_kino_state()` — best-effort: opens the repo,
    extracts HEAD's `.kinora/` to a scratch dir, and compares it to the working
    copy. Returns false (no warning) on any error or when there's nothing to
    compare (non-git repo, unborn HEAD, no committed `.kinora/`). Never blocks
    or fails the render.
  - `dirs_differ` / `collect_rel_files` recursive byte-comparison helpers.
- `crates/kinora-cli/src/main.rs`: prints the advisory warning to stderr.

Tests: `dirs_differ` add/remove/change + recursion; render flags uncommitted
state; render stays quiet when committed; render stays quiet for a non-git
repo. Full workspace suite green, zero warnings.
