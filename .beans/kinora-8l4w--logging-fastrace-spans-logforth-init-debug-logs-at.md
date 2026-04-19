---
# kinora-8l4w
title: 'Logging: fastrace spans + logforth init + debug logs at silent error sites'
status: completed
type: task
priority: normal
created_at: 2026-04-19T13:33:45Z
updated_at: 2026-04-19T13:47:23Z
---

Wire fastrace + logforth into kinora-cli so library calls emit structured spans and swallowed errors surface as debug logs. Keeps the nominal error enums unchanged — error-handling migration to rootcause is deferred to a follow-up bean.

## Scope

### In scope

- [x] Deps wiring: `kinora` gets `log` + `fastrace` (no `enable` feature); `kinora-cli` gets `log`, `fastrace` with `enable`, `logforth`, `logforth-append-fastrace`.
- [x] Instrument library entry points with `#[fastrace::trace]`:
  - `compact::compact_root`, `compact::compact_all`, `compact::compact_root_with_refs`, `compact::ExternalRefs::collect`
  - `kino::store_kino`, `resolve::Resolver::load`, `init::init`, `ledger::Ledger::read_all_events`
- [x] Convert silent `Err(_) => continue` swallows in `ExternalRefs::collect` (5 sites) to `log::debug!` with source root + error context.
- [x] Add `log::warn!` at the two `parse_ts` fallback sites in `apply_root_entry_gc` and `prune_hot_events`.
- [x] CLI init (`main.rs`): `logforth::starter_log` with `Stderr` appender + `EnvFilter::from_default_env()` (RUST_LOG). Optional `fastrace` reporter enabled via `KINORA_TRACE=1` env var. Root span wraps the CLI dispatch. `fastrace::flush()` before every return path.
- [x] Zero compiler warnings.

### Out of scope (deferred)

- Error-handling migration to `rootcause` (follow-up bean).
- `--trace` CLI flag (env-var gating is enough for now).
- Span-assertion tests — testing `log`/`fastrace` output is fraught (global singletons, test parallelism); this task is pure infrastructure with no behavior change, so existing 301+81 tests plus manual `RUST_LOG=debug kinora compact` smoke cover it.

## Acceptance

- [x] All existing tests pass
- [x] Zero compiler warnings
- [x] `RUST_LOG=debug` surfaces `ExternalRefs::collect` skip logs
- [x] `KINORA_TRACE=1` produces fastrace console output
- [x] Bean todo items all checked off
- [ ] Summary of Changes section added at completion

## Plan

### Commit sequence

1. `feat(log): wire fastrace + logforth; instrument library entry points`
2. `feat(log): debug/warn at silent swallow sites in compact`
3. (optional) review fix

### Notes

- `fastrace::enable` feature: only `kinora-cli` activates it. Cargo feature unification means the library's `#[fastrace::trace]` calls will still emit spans when the CLI runs.
- `logforth-append-fastrace::FastraceEvent::exit()` is a no-op and fastrace spans don't auto-flush on drop — explicit `fastrace::flush()` needed at every CLI return path.
- Root span is created in `main()` so every library-level `#[trace]` fn attaches as a child.

## Summary of Changes

### Logging infrastructure

- Added `log` + `fastrace` deps to `kinora`. The library uses the default (no-op unless caller opts in) feature set, so spans have zero cost when the library is used without a CLI init.
- Added `log`, `fastrace` (with `enable` feature), `logforth` (with `starter-log`/`append-fastrace`/`diagnostic-fastrace` features), and `logforth-append-fastrace` deps to `kinora-cli`. Cargo feature unification means the `enable` activation propagates down: the library's `#[fastrace::trace]` attributes emit spans when driven from the CLI.
- Wired `logforth::starter_log::builder()` in `kinora-cli/src/main.rs` with two dispatches: (1) `FastraceEvent` so log events are recorded into the active span, and (2) `Stderr` with `FastraceDiagnostic` + `EnvFilterBuilder::from_default_env_or("info")` — `RUST_LOG` overrides the default info gate.
- `KINORA_TRACE=1` env var installs `ConsoleReporter` so fastrace dumps spans to stderr on `flush()`. Opt-in only to keep normal runs quiet.
- `main()` creates a single root span (`kinora.main`) covering every command; `fastrace::flush()` fires before `ExitCode` return. Every library-level `#[trace]` fn attaches as a child.

### Instrumented library entry points

`#[fastrace::trace]` applied to: `compact::{compact_root, compact_all, compact_root_with_refs, ExternalRefs::collect}`, `kino::store_kino`, `resolve::Resolver::load`, `init::init`, `ledger::Ledger::read_all_events`.

### Swallow-site logs

- `ExternalRefs::collect` had 5 `Err(_) => continue` swallows (pointer read, blob read, root parse, hash parse, kinograph parse, pick_head). All now `log::debug!` to `kinora::compact::refs` with source root + error context before continuing. The best-effort semantics are preserved — cross-root integrity still degrades gracefully for unresolvable sibling roots — but debugging is no longer a guessing game.
- Two `parse_ts` fallbacks in `apply_root_entry_gc` and `prune_hot_events` now `log::warn!` to `kinora::compact::gc` / `kinora::compact::prune` with the offending ts, before the conservative "keep the event" fallback. Bad timestamps in the wild should be rare but visible.

### What's next

Error-handling migration to `rootcause` is a separate deferred bean. The `CompactError`/`StoreError`/etc. enums are unchanged here.
