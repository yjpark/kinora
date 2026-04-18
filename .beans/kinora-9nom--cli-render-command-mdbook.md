---
# kinora-9nom
title: 'CLI: render command (mdbook)'
status: in-progress
type: feature
priority: normal
created_at: 2026-04-18T09:16:59Z
updated_at: 2026-04-18T16:54:14Z
parent: kinora-w7w0
blocked_by:
    - kinora-zboo
    - kinora-6zxd
---

Scans local branches and worktrees, resolves kinos and kinographs from each branch's `.kinora/`, produces mdbook output at the cache path.

RFC-0003 sections: *Rendering*, *Minimal CLI → render*. Design decisions in `kinora-fhw1`.

## Design

### Cache path derivation

- Read `.kinora/config.styx → repo-url` (required; error if absent)
- Normalize: strip scheme, user-info, `.git`, trailing `/`; SSH `:` → `/`; lowercase host (path preserved)
- `shorthash` = first 8 hex chars of `BLAKE3(normalized-url)`
- `name` = last path segment of normalized URL, sanitized to `[a-z0-9_-]`
- **Cache path:** `~/.cache/kinora/<shorthash>-<name>/`

### Scanning

1. Enumerate local branches and worktrees
2. For each, read `.kinora/ledger/*.jsonl` visible in that branch's tree
3. Union all events
4. Resolve kinos and kinographs
5. MVP: each branch rendered as a top-level section in SUMMARY.md

### Kind dispatch (MVP)

- `markdown`: render content directly; parse `kino://<id>/` URLs → cross-links to rendered pages
- `kinograph`: concatenate referenced kino contents in order; per-entry notes as blockquotes
- `text`: plain passthrough (fenced code block)
- `binary`: skip with "opaque binary — see source" marker
- Other kinds: skip with warning

### Output layout

```
~/.cache/kinora/<shorthash>-<name>/
  book.toml
  src/
    SUMMARY.md                 # organized by branch
    <branch>/
      <kino-id>.md
      <kinograph-id>.md
```

### Rebuilds

- Full rebuild on every run (no incremental)
- Safe to delete the cache directory; next render regenerates everything

## Acceptance

- [x] Reads `.kinora/config.styx → repo-url`; errors if absent
- [x] Derives cache path correctly per normalization rules
- [x] Renders single current branch end-to-end
- [~] Extends to all local branches and worktrees (union of ledger files per branch) — **deferred** to kinora-ohwb
- [x] Kind dispatch: `markdown` + `kinograph` in MVP
- [x] `kino://<id>/` URLs resolved to cross-links between rendered pages
- [x] SUMMARY.md organized by branch
- [x] Source markers include originating branch
- [x] Full rebuild on every run
- [x] Output is viewable via `mdbook serve` from cache path (verified via `mdbook build` smoke test — HTML output generated cleanly)


## Plan

Library-first. Four layers, each test-driven:

**1. `crates/kinora/src/cache_path.rs`** — pure function
- `CachePath::from_repo_url(url) -> CachePath { shorthash, name }`
- Normalize: strip scheme, user-info, `.git`, trailing `/`; SSH `host:path` → `host/path`; lowercase host (path preserved)
- `shorthash` = first 8 hex of BLAKE3(normalized)
- `name` = last path segment sanitized to `[a-z0-9_-]` (drop empties, collapse repeats)
- `CachePath::subdir() -> String` returns `<shorthash>-<name>`

**2. `crates/kinora/src/render.rs`** — in-memory render
- `RenderedPage { id, slug, branch, body }` — one per kino
- `Book { pages: Vec<RenderedPage> }` — ordered
- `render_for_branch(resolver, branch_label) -> Result<Book, RenderError>`:
  - Walk each identity's current head via `Resolver::identities()` + `pick_head`
  - Kind dispatch:
    - `markdown` → body = resolved content as UTF-8
    - `kinograph` → body = `Kinograph::render(resolver)` output
    - `text` → body = fenced ```` ```text … ``` ```` block
    - `binary` → body = `> (opaque binary — see source)` blockquote
    - other → emit `> (unrenderable kind: …)` with warning
  - Resolve `kino://<64hex>/` URL occurrences in final body → rewrite to relative page path
- `RenderError` variants: Resolve, Kinograph, Utf8

**3. `crates/kinora/src/render.rs` disk writer**
- `write_book(book, cache_root, books: Vec<(branch, Book)>)`:
  - Rebuild from scratch (rm existing `src/`)
  - Emit `book.toml` with title from repo-url name
  - Emit `src/SUMMARY.md` grouped by branch
  - Emit `src/<branch>/<slug>.md` each with a source marker footer `*From branch `<branch>`*`

**4. CLI `crates/kinora-cli/src/render.rs`**
- `run_render(cwd) -> Result<RenderReport, CliError>`
- MVP scope for this bean:
  - Single-branch render using current-lineage resolver
  - Multi-branch + worktree enumeration **deferred to a follow-up bean** (see blocker note below). The single-branch path emits `SUMMARY.md` organized with a single top-level branch group named after `HEAD`'s lineage so the layout is forward-compatible.
- Prints `wrote <N> pages under <path>` on success.

**Why defer multi-branch?**
Multi-branch = walking `gix` tree objects for each local ref to read `.kinora/ledger/*.jsonl` snapshots at that commit. That's a significant gix dive (repo.references(), iter local branches, branch.peel_to_tree(), tree.lookup_entry_by_path(".kinora/ledger"), iterate tree entries, read blobs). Worktrees adds `repo.worktrees()` enumeration. Each path needs its own Ledger parser feeding a per-branch Resolver. This is another well-scoped chunk that deserves its own bean with tests against fixture repos. The library layers above are designed to accept `Vec<(branch, Resolver)>` so the wiring is straightforward once that work lands.

## Commits
1. cache_path library + tests
2. render library (in-memory Book + markdown/kinograph + kino:// URL rewrite) + tests
3. disk writer + tests
4. CLI `render` command + end-to-end tests
5. review fixes if any
