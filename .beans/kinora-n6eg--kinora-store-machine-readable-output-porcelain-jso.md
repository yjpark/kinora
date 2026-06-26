---
# kinora-n6eg
title: 'kinora store: machine-readable output (--porcelain/--json)'
status: completed
type: task
priority: normal
created_at: 2026-06-26T00:48:24Z
updated_at: 2026-06-26T03:54:07Z
---

kinora store prints id=<hash> (with '='), easy to mis-parse vs the styxl 'id <hash>' (space) form; cost a set -e exit when scripting. Provide stable machine-readable output (--porcelain or --json) for store (and audit other commands for the same).

## Summary of Changes

Added a global `--json` flag (`Cli.json` in cli.rs) and a stable
machine-readable output for `kinora store`:

- `crates/kinora-cli/src/store.rs`: `StoreJson` struct + `format_store_json`,
  emitting a single-line JSON object `{kind,id,hash,event,new_event}`. Solves
  the `id=<hash>` mis-parse footgun (no more `=`-delimited parsing under
  `set -e`). Uses `facet_json` with a safe hand-built fallback (all fields are
  constrained: hex hashes, namespace-validated kind, bool).
- `crates/kinora-cli/src/main.rs`: store branch emits JSON when `--json` is set.
- `crates/kinora-cli/Cargo.toml`: added `facet-json` dependency.

Scope note: `--json` is currently honored by `store` (the reported pain point).
Other commands ignore it for now — wiring the remaining commands (assign,
commit, resolve, render) is a follow-up; documented on the flag's help text.

Tests: `format_store_json_emits_parseable_fields` (round-trips through a JSON
parser, asserts no `key=value` form) and `format_store_json_reflects_idempotent_restore`.
All 117 kinora-cli tests pass, zero warnings.
