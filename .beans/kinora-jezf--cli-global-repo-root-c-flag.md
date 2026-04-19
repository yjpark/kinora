---
# kinora-jezf
title: 'CLI: global --repo-root / -C flag'
status: todo
type: task
priority: normal
created_at: 2026-04-19T14:50:35Z
updated_at: 2026-04-19T15:29:55Z
---

## Why

All commands currently start with `find_repo_root(cwd)` (see `kinora-cli/src/common.rs`), which walks up from the current directory looking for `.kinora/`. That's fine for interactive use but awkward for:

- Tests that drive the CLI against a tempdir
- Scripts operating on multiple repos
- The upcoming `clone` and `repack` commands, which need to target an arbitrary `.kinora/` path

A git-style `-C <path>` / `--repo-root <path>` flag solves all three cleanly.

## Semantics

- If `--repo-root` is given, use it verbatim as the repo root. Validate that `.kinora/` exists under it; error if not (same error type as `NotInKinoraRepo` today).
- If omitted, keep current behavior: walk up from cwd.
- Flag is global — accepted before or after the subcommand, visible on every subcommand's help.

## Areas affected

- `kinora-cli/src/cli.rs` — add the flag to the top-level `Cli` struct
- `kinora-cli/src/main.rs` — replace the `std::env::current_dir()` call with: if flag present use it, else use cwd, then still call `find_repo_root` on the result
- `kinora-cli/src/common.rs` — nothing to change; `find_repo_root` already takes a `&Path`

## Todos

- [ ] Add `--repo-root` / `-C` to top-level CLI
- [ ] Wire it into the path resolution in `run()`
- [ ] Test: CLI against a tempdir via `-C` resolves correctly
- [ ] Test: `-C /nonexistent` errors with `NotInKinoraRepo`
- [ ] Update any existing docs/examples that assume cwd-only

## Acceptance

- Flag works on all subcommands (store, assign, render, compact, resolve)
- Zero warnings, all tests pass
- No behavior change when flag is absent
