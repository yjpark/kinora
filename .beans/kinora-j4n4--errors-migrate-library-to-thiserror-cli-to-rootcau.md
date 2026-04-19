---
# kinora-j4n4
title: 'Errors: migrate library to thiserror; CLI to rootcause'
status: in-progress
type: task
priority: normal
created_at: 2026-04-19T14:01:15Z
updated_at: 2026-04-19T14:08:44Z
---

Pairing thiserror (library, pure-typed) with rootcause (CLI, report/render) is the 80% win with minimal library impact. The library errors already implement `std::error::Error` manually and flatten inner errors into Display strings — thiserror's `#[from]` gives us proper `source()` chains for free.

## Scope

### Library — 15 enums across these files

- [x] `assign.rs` — AssignError
- [x] `compact.rs` — CompactError
- [x] `config.rs` — ConfigError
- [x] `event.rs` — EventError
- [x] `hash.rs` — HashParseError
- [x] `init.rs` — InitError
- [x] `kinograph.rs` — KinographError
- [x] `kino.rs` — StoreKinoError
- [x] `ledger.rs` — LedgerError
- [x] `namespace.rs` — NamespaceError
- [x] `render.rs` — RenderError
- [x] `resolve.rs` — ResolveError (careful: `MultipleHeads` carries custom fields the CLI pattern-matches)
- [x] `root.rs` — RootError
- [x] `store.rs` — StoreError
- [x] `validate.rs` — ValidationError

Each enum should keep the same Display output as today (tests may assert it). Use:
- `#[error("literal {field}")]` — Display with interpolation
- `#[error(transparent)]` + `#[from]` — pure wrappers
- `#[from]` — auto `From<E>` AND `source()`
- `#[source]` without `#[from]` — when we want `source()` but From would collide

### CLI

- [ ] Add `rootcause` dep to `kinora-cli` (workspace already declares it).
- [ ] Convert `CliError` to `thiserror`.
- [ ] At command dispatch sites in `main.rs`, wrap the `CliError` in `rootcause::Report` and attach command-level context.
- [ ] Replace `eprintln!("error: {e}")` → `eprintln!("{report:?}")`.
- [ ] Preserve special-cased renderers (`MultipleHeads` fork report, `AmbiguousAssign` D2 hint) — detect those variants before wrapping.

## Plan

### Commit sequence

1. `refactor(errors): migrate library error types to thiserror` — mechanical sweep, zero behavior change, tests still pass.
2. `feat(cli): rootcause reports with per-command context` — replaces Display flattening with rootcause pretty output, attaches context at boundaries.

### Notes

- Some existing Display impls wrap the inner message with a prefix. With `#[error(transparent)]` that prefix disappears — CLI tests that check exact stderr strings need updating.
- `CompactError::AmbiguousAssign` and `ResolveError::MultipleHeads` are pattern-matched by the CLI for custom rendering; keep their shape.

## Acceptance

- [ ] All existing tests pass
- [ ] Zero compiler warnings
- [ ] `RUST_LOG` / `KINORA_TRACE` still work
- [ ] CLI error output shows chained cause ≥1 level deep on a compound error
- [ ] Bean todo items all checked off
- [ ] Summary of Changes section added at completion
