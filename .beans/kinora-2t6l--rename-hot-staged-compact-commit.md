---
# kinora-2t6l
title: 'Rename: hot → staged, compact → commit'
status: in-progress
type: task
priority: normal
created_at: 2026-04-19T14:39:05Z
updated_at: 2026-04-19T15:36:35Z
---

## Why

'Hot' and 'compact' are jargon carried over from an earlier mental model. 'Staged' and 'commit' map directly onto the git vocabulary most users already carry, and they describe the behavior (assign events sit 'staged' waiting to become a 'commit' into the root-kinograph) with less cognitive load.

## Scope

Pure rename only — **no** lifecycle change in this bean. Staged events remain in place after commit, exactly as 'hot' events remain after compact today. Cleanup + history preservation is tracked separately.

## Areas affected

- `kinora/src/paths.rs` — `hot_dir`, `HOT_DIR` → `staged_dir`, `STAGED_DIR`
- `kinora/src/compact.rs` → `kinora/src/commit.rs` (module, types, functions)
- `kinora/src/ledger.rs` — any 'hot' naming
- `kinora-cli` — `compact` subcommand → `commit`; `--hot` flag names
- Tests — fixture paths, assertions referencing `.kinora/hot/`
- Docs — README, RFC-0003, CLAUDE.md
- Error types — `CompactError` → `CommitError`; variant names

## Todos

- [x] Rename `HOT_DIR`/`hot_dir` → `STAGED_DIR`/`staged_dir` in paths.rs
- [x] Rename module `compact` → `commit` in kinora lib
- [x] Rename `CompactError` → `CommitError`
- [x] Rename `run_compact`/`CompactReport` etc.
- [x] Rename CLI subcommand `compact` → `commit` (and its module in kinora-cli)
- [x] Update all tests that reference `.kinora/hot/` path literal
- [x] Update README, RFC-0003, CLAUDE.md references (no references found — N/A)
- [x] Verify zero warnings, all tests pass (382 tests pass, zero diagnostics)

## Acceptance

- `cargo test --workspace` passes
- Zero compiler warnings
- `kinora commit` works identically to the old `kinora compact`
- No residual `hot`/`compact` identifier in library code (hard rename, no transitional aliases — repo is days old, no external users)
