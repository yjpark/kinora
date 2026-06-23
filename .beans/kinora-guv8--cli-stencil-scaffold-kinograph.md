---
# kinora-guv8
title: 'CLI: stencil scaffold <kinograph>'
status: completed
type: feature
priority: normal
created_at: 2026-06-06T09:28:42Z
updated_at: 2026-06-23T00:45:20Z
parent: kinora-bm7z
blocked_by:
    - kinora-hgpl
---

Given an api-kinograph (by name or id), generate a new source file: stencil:kinograph header + one stencil:slot + filled read-only block per entry, in kinograph order, via the engine + RustTarget. First-placement helper; subsequent edits are agent-driven. TDD/integration.

## Plan

Implement `stencil scaffold <kinograph>` by reusing the engine.

### Design decisions
- **Output → stdout.** The CLI surface is just the `kinograph` positional (no output path). Printing the generated source to stdout is composable (`stencil scaffold user-api > src/user.rs`) and sidesteps filename-guessing / clobber semantics. A `-o/--output` flag is a trivial follow-up if wanted.
- **Reuse `sync_file`.** Build a skeleton source (`stencil:kinograph` header + one `stencil:slot <name>` per entry, in kinograph order, blank-line separated), parse it, and run `sync_file` so the read-only blocks fill through the identical engine path. Guarantees scaffold output re-syncs clean.

### Engine change (crates/stencil/src/engine.rs)
- Factor `build_index` into a shared `resolve_entries(reference, resolver) -> Vec<(String, Resolved)>` that preserves kinograph (document) order, skips unresolvable entries (tolerant), and errors on duplicate names. `build_index` = collect into BTreeMap.
- Add `pub fn kinograph_slot_names(reference, resolver) -> Result<Vec<String>, StencilError>` = ordered names from `resolve_entries`. New public API for scaffold.

### CLI (crates/stencil-cli/src/scaffold.rs)
- `run_scaffold(repo_root, reference, target) -> Result<String, CliError>`: load Resolver, `kinograph_slot_names`, build skeleton via `target.comment_leader()`, parse + `sync_file`, return `to_source`.
- main.rs: wire `Command::Scaffold { kinograph }` → print the string to stdout.

### Tests (TDD)
- engine: `kinograph_slot_names` preserves order / skips unresolvable / errors on dup / rejects non-kinograph.
- scaffold: header + one slot+ro per entry in order; output re-syncs clean (idempotent); empty kinograph → header only; unknown kinograph errors; non-kinograph kind errors.

## Summary of Changes

Implemented `stencil scaffold <kinograph>` in `crates/stencil-cli/src/scaffold.rs`, reusing the engine end to end.

- **`run_scaffold(repo_root, reference, target)`** loads the Resolver, gets ordered entry names via the new `engine::kinograph_slot_names`, builds a skeleton (binding header + one blank-line-separated `stencil:slot` per entry), parses it, runs `sync_file` to fill the read-only blocks, and returns the rendered source. Output goes to **stdout** (`stencil scaffold user-api > src/user.rs`) — composable, no filename-guessing or clobber.
- **Engine refactor (`engine.rs`):** factored `build_index` into a shared `resolve_entries(reference, resolver) -> Vec<(name, Resolved)>` preserving kinograph (document) order, tolerant per-entry, fail-loud on duplicate names. Added `pub fn kinograph_slot_names` returning ordered names — routes through the same `resolve_entries`, so every scaffolded slot name is exactly one `sync_file` will match (no `Unmatched` surprise). Verified behavior-preserving for `build_index`.
- **`main.rs`** wires `Command::Scaffold { kinograph }` → print to stdout; removed the now-stale `NotImplemented` CliError variant and the stale `#[allow(dead_code)]` / "lands in bean X" notes on the CLI enum (both commands are live).

**Tests:** 6 scaffold CLI tests (header + filled slot per entry in kinograph order; output re-syncs clean for by-name and by-id; empty kinograph → header only; unknown/non-kinograph errors; unslottable-name error) + 5 engine tests for `kinograph_slot_names` (order preserved, skips unresolvable, duplicate error, non-kinograph rejected, whitespace-name error). Full workspace green (391 + 115 + 72 + 26), zero warnings, clippy clean.

A fresh-eyes review flagged that an entry name with whitespace would make scaffold emit an unparseable `stencil:slot` marker and fail with an opaque parser error. Fixed by adding `StencilError::UnslottableEntryName`, which `kinograph_slot_names` raises loudly for empty/whitespace names (a slot marker is a single token); pinned with engine + CLI tests. Also added the missing by-id re-sync coverage.
