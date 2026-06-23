---
# kinora-exay
title: 'CLI: stencil sync <paths>'
status: completed
type: feature
priority: high
created_at: 2026-06-06T09:28:42Z
updated_at: 2026-06-23T00:37:22Z
parent: kinora-bm7z
blocked_by:
    - kinora-hgpl
---

Scan given paths (files or dirs, recursive; default cwd) for stencil markers, apply the engine, and write changed files atomically. Report: files changed, slots filled/refreshed, drift warnings, entries with no slot. Non-zero exit on errors (unknown slot, parse failure). TDD/integration.

## Plan

Implement `stencil sync <paths>` as a testable CLI module `crates/stencil-cli/src/sync.rs`, driving the engine over real files.

### Surface
- `run_sync(repo_root, cwd, paths, target) -> Result<SyncSummary, CliError>` — repo-level setup errors (resolver load, missing path arg) are fatal `Err`; per-file errors are collected into the summary.
- `collect_files(cwd, paths)` — resolve each path arg against cwd. File arg → included verbatim. Dir arg → recursive walk, `.rs` only, skipping dot-dirs and `target/`. Missing path → fatal `CliError::PathNotFound`. Dedup+sort via `BTreeSet`.
- `sync_one(path, resolver, target) -> FileOutcome` — read_to_string, `StencilFile::parse`, `sync_file`; if `report.changed()`, render `to_source` and `atomic_write`. Any `StencilError` (io/parse/resolve) collected as the file outcome's `result: Err`.
- `atomic_write(path, bytes)` — write `.stencil-tmp-<name>` sibling, then `fs::rename` (mirrors kinora `Store::write`).
- `format_sync_summary(&SyncSummary)` — human report: files changed, per-file created/updated counts, drift warnings, unslotted entries, errors.

### Exit code
`SyncSummary::has_errors()` = any file `result.is_err()` OR any report has unmatched slots (unknown slot). main.rs: non-zero exit when `has_errors()`. Changed-but-valid files are still written.

### Deps
Re-add `kinora = { path = "../kinora" }` (Resolver, paths::kinora_root) — removed in vsuo as unused, needed now. Add `tempfile` + `kinora` test helpers (init/store_kino) to dev-deps for integration tests.

### Tests (TDD, unit tests in sync.rs mirroring kinora-cli convention)
fills+writes a slot; second run 0 changed; drift reported; unknown slot → has_errors; parse failure → per-file error; dir walk finds .rs recursively + skips target/; unslotted reported; missing path → fatal.

## Summary of Changes

Implemented `stencil sync <paths>` in `crates/stencil-cli/src/sync.rs` — the file-I/O and path-walking shell around the pure `stencil::engine`.

- **`run_sync(repo_root, cwd, paths, target)`** loads the kinora `Resolver`, collects files, syncs each, and returns a `SyncSummary`. Repo-level failures (resolver load, missing path arg) are fatal `Err`; per-file failures (parse/resolve/io) are collected into the summary so one bad file never aborts the run.
- **`collect_files` + `walk_dir`** resolve path args against cwd (empty → cwd default); files taken verbatim, directories walked recursively for `*.rs`, skipping `target/` and dot-dirs. Dedup is by *canonical* identity (so `sub/a.rs`, `./sub/a.rs`, and the dir `sub` collapse to one) while keeping the original spelling for display. Symlinks deliberately not followed (cycle-free walk; documented). Missing path → fatal `CliError::PathNotFound`.
- **`atomic_write`** stages to a `.stencil-tmp-<name>` sibling then `fs::rename`s over the target, mirroring kinora `Store::write`.
- **`format_sync_summary`** renders a header + per-file slot tallies (created/updated/drift) + drift warnings, unmatched-slot errors, orphan-block warnings, unslotted-entry warnings, and per-file hard errors.
- **Exit code:** `SyncSummary::has_errors()` (any per-file error OR unknown/unmatched slot) drives a non-zero exit in `main.rs`; changed-but-valid files are still written.

Supporting changes: re-added the `kinora` dep to `stencil-cli` (Resolver/paths, dropped in vsuo) + `tempfile` dev-dep; added `find_repo_root`/`resolve_repo_root` and `Io`/`Resolve`/`NotInKinoraRepo`/`PathNotFound` variants to `common.rs` (mirrors kinora-cli); wired repo-root resolution and the `Sync` dispatch in `main.rs`.

**Tests:** 19 unit tests in stencil-cli (was 0 sync tests) — fill+write, idempotent no-op, drift overwrite, unknown-slot error, parse-failure isolation, recursive walk skipping target/, unslotted reporting, missing-path fatal, empty-paths default, absolute-path arg, duplicate-spelling dedup, and one test per summary-formatting branch (header/tally/drift/unmatched/unslotted/orphan/error). Full workspace green (391 + 115 + 67 + 19), zero warnings, clippy clean.

A fresh-eyes code review (subagent) flagged: orphan blocks were dropped from the summary (now surfaced as warnings), path dedup missed alternate spellings (now canonical-keyed), undocumented symlink skip (now documented), and several formatting/coverage gaps (now tested). All addressed in this change.
