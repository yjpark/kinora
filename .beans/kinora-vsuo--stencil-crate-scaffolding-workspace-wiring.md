---
# kinora-vsuo
title: Stencil crate scaffolding + workspace wiring
status: completed
type: feature
priority: high
created_at: 2026-06-06T09:28:31Z
updated_at: 2026-06-06T09:36:12Z
parent: kinora-bm7z
---

Create crates/stencil (lib) and crates/stencil-cli (bin stencil); add to workspace members; depend on the kinora crate; wire logging (logforth+fastrace) and errors (thiserror in lib, rootcause in cli) matching kinora conventions. Document the kudo::api-spec / kudo::api-kinograph kind conventions in the lib. No kinora-crate changes.

## Summary of Changes

Scaffolded the two stencil crates and wired them into the workspace.

- **`crates/stencil`** (lib, depends on `kinora`): crate-level docs explaining
  stencil + the kind conventions; `kinds` module with `API_SPEC`
  (`kudo::api-spec`) and `API_KINOGRAPH` (`kudo::api-kinograph`) constants;
  `StencilError` (thiserror) with the foundational `From` conversions for the
  kinora errors stencil builds on (`io`, `ResolveError`, `KinographError`).
  Tests assert the kinds' spelling and that they pass `kinora`'s namespace
  validation unchanged.
- **`crates/stencil-cli`** (bin `stencil`): logforth + fastrace logging wired
  exactly like kinora-cli (`STENCIL_TRACE=1` opt-in reporter); figue `Cli` with
  `-C/--repo-root` and a `Command` enum declaring the `sync` and `scaffold`
  surface; `CliError` (wraps `StencilError`) reported via `rootcause`. The
  command engines are stubbed with `NotImplemented` pending kinora-hgpl /
  kinora-exay / kinora-guv8.
- **Workspace**: added both crates to `members`. No `kinora`-crate changes —
  the namespaced kinds need no registration.

Acceptance: all tests pass, zero compiler warnings, `stencil --help` renders,
stubs report cleanly.
