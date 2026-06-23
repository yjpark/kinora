---
# kinora-se7b
title: 'Dogfood: define stencil''s own public API as a kinograph and rebuild stencil src from it'
status: completed
type: task
priority: high
created_at: 2026-06-06T09:28:42Z
updated_at: 2026-06-23T00:56:32Z
parent: kinora-bm7z
blocked_by:
    - kinora-exay
    - kinora-guv8
---

Author kudo::api-spec kinos for stencil's public surface (markdown+fenced-rust); compose a kudo::api-kinograph; place stencil:kinograph + stencil:slot markers in stencil's source; run stencil sync; verify the read-only blocks match, the crate still compiles, and a second sync is a no-op. RFC day-one dogfood. Blocked by the engine + CLI.

## Plan & scoping decision

29 public items span the crate. To prove the RFC day-one dogfood loop end-to-end on real stencil source — with real `kinora store` + real `stencil sync` against this repo's `.kinora` — without a high-churn blind restructure of the two 700-line modules, I scope THIS bean to a complete, rigorous dogfood of `crates/stencil/src/target.rs` (the smallest self-contained public module: the `LanguageTarget` trait + `RustTarget` struct). The remaining modules follow the identical convention and get a tracked follow-up bean.

### Dogfood convention (established here)
- **One `kudo::api-spec` kino per public item.** Content = markdown: the item's doc prose, then a fenced ```rust block carrying the definition.
- **Whole-item contracts** (trait, struct, enum, const, type): the fenced rust is the complete definition; the read-only block is the entire item, no editable region follows.
- **Functions/methods** (follow-up modules): the fenced rust is the signature WITHOUT a trailing `;` or body; the `stencil:slot` is placed immediately before the `{` body, so the read-only block owns doc+signature and the body stays editable. (`pub fn f() -> T` followed by `{ ... }` is valid Rust.)
- Names are kebab-case `module-item`; the slot name must equal the spec kino's metadata `name`.

### Steps
1. Author api-spec kinos: `target-language-target`, `target-rust-target` (markdown+fenced-rust matching current source).
2. `kinora store -k kudo::api-spec -n <name> --provenance dogfood-se7b < spec.md` → capture ids.
3. Author `kudo::api-kinograph` named `stencil-target-api` (styxl `entries (...)` referencing the two ids) and store it.
4. Edit target.rs: add `// stencil:kinograph stencil-target-api` binding + a `// stencil:slot <name>` where each item was (removing the originals).
5. `stencil sync crates/stencil/src/target.rs` → fills read-only blocks.
6. Verify: blocks reproduce the items, `cargo test -p stencil` passes, bacon clean.
7. Second `stencil sync` → no-op (Unchanged).
8. Follow-up bean for engine/region/spec/kinds/lib.

All steps are git-reversible (`.kinora` store + source are tracked, nothing pushed).

## Summary of Changes

Proved the RFC-0004 day-one dogfood loop end-to-end on real stencil source, using the real `kinora` and `stencil` CLIs against this repo's `.kinora` store.

**What was done (target.rs slice):**
1. Authored two `kudo::api-spec` kinos (markdown prose + fenced ```rust) for stencil's `target.rs` public surface:
   - `target-language-target` (id `1a2a5f0a…`) — the `LanguageTarget` trait.
   - `target-rust-target` (id `377dd98f…`) — the `RustTarget` struct.
2. Composed a `kudo::api-kinograph` `stencil-target-api` (id `a6bce71e…`) referencing both, via styxl `{id …}` entries.
3. Restructured `crates/stencil/src/target.rs` to the in-place slot model: a `// stencil:kinograph stencil-target-api` binding + a `// stencil:slot <name>` where each item was (originals removed; the `impl` and tests left as editable regions).
4. Ran `stencil sync crates/stencil/src/target.rs` → 2 read-only blocks created, reproducing the trait and struct from the kinos.

**Verification (all acceptance criteria met):**
- Read-only blocks match the source — confirmed first via `stencil scaffold stencil-target-api` (dry run) and then the in-place sync.
- Crate compiles; all 72 stencil tests pass; full workspace green (391+115+72+26); bacon + clippy clean.
- Second `stencil sync` reports `0 changed` — idempotent no-op.
- The synced read-only blocks are rustfmt-neutral (the only `target.rs` fmt diff is a pre-existing long `assert_eq!` in the test module).

**Convention established** (recorded in the bean Plan and carried into the follow-up): one api-spec kino per public item; whole-item contracts (trait/struct/enum/const/type) render the full definition as the read-only block; functions split doc+signature (no trailing `;`/body) from the editable `{ body }`.

**Scope:** this bean proves the loop on `target.rs` (the smallest self-contained module). The remaining 27 public items across engine/region/spec/kinds/lib follow the identical pattern and are tracked in follow-up **kinora-3guj** — that bean also first exercises the fn signature/body split and decides kinograph granularity (per-module vs crate-wide).

**Repo state:** the 3 dogfood kinos live as staged ledger events (the Resolver reads staged, so sync resolves them without a commit); a `kinora commit` would fold them into the `inbox` root when desired. All changes are git-tracked and were not pushed.
