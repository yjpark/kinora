---
# kinora-3guj
title: 'Dogfood: extend stencil api-kinograph to engine/region/spec/kinds/lib'
status: in-progress
type: task
priority: normal
created_at: 2026-06-23T00:56:17Z
updated_at: 2026-06-23T01:10:58Z
parent: kinora-bm7z
---

Extend the kinora-se7b dogfood from target.rs to the rest of stencil's public surface (27 remaining items across engine.rs, region.rs, spec.rs, kinds.rs, lib.rs), following the convention established in se7b.

## Convention (from kinora-se7b)
- One kudo::api-spec kino per public item: markdown prose (the item's doc) + a fenced rust block carrying the definition.
- Whole-item contracts (struct/enum/trait/const/type): fenced rust is the complete definition; the read-only block is the whole item, no editable region follows.
- Functions/methods: fenced rust is the signature WITHOUT a trailing semicolon or body; the stencil:slot sits immediately before the body brace so the read-only block owns doc+signature while the body stays editable (pub fn f() -> T then { ... } is valid Rust). NOT yet exercised — engine/region/spec methods are where it first applies; verify it compiles and re-syncs clean.

## Acceptance
- api-spec kinos authored for all remaining public items; markers placed; stencil sync fills all blocks; crate compiles; all tests pass; second sync is a no-op.

## Open questions surfaced by se7b
- Kinograph granularity: per-module vs one crate-wide stencil-api kinograph.
- Validate the fn signature/body split on real multi-line signatures (e.g. engine::sync_file).
- Consider kinora commit to fold the api-spec kinos from staged into a root once the full surface is captured.

## Progress

Resolved open question — **kinograph granularity = per-module/per-file**. The in-place file-binding model means each source file binds one kinograph; a single crate-wide kinograph bound by one file would make every other file report all foreign items as unslotted. So: one kinograph per module (`stencil-target-api`, `stencil-spec-api`, …). A crate-wide composition can later layer over these if needed.

### Increment 1: spec.rs ✓ (validated the fn signature/body split)
- [x] api-spec kinos: spec-item (struct), spec-error (enum), spec-item-parse, spec-item-from-bytes, spec-item-code, spec-item-has-code (methods — fn split)
- [x] kinograph stencil-spec-api
- [x] markers in spec.rs; sync (6 created); compiles; 72 tests pass; second sync no-op

**Validated:** the fn signature/body split (`pub fn f() -> T` read-only, `{ body }` editable) compiles and re-syncs clean. Also surfaced + handled the **4-backtick fence** case: `SpecItem`'s field doc contains ` ```rust `, so its spec kino wraps the definition in a 4-backtick fence — pulldown-cmark + SpecItem::parse handle it, rendering the triple-backtick text verbatim into source.

### Increment 2: kinds.rs + lib.rs ✓ (whole-item)
- [x] kinds.rs: kinds-api-spec, kinds-api-kinograph consts → stencil-kinds-api kinograph; sync (2 created); no-op.
- [x] lib.rs: stencil-error enum → stencil-lib-api kinograph; sync (1 created); no-op.

**Finding (positive):** a doc comment containing ` ```rust ` *in prose* (kinds API_SPEC) renders correctly — pulldown-cmark treats the unclosed inline backticks as literal text (the paragraph ends before the real fence), so SpecItem::parse extracts the real fenced block cleanly and the prose reproduces verbatim. The content model handles backtick-bearing docs both in prose (inline) and inside definitions (4-backtick fence, validated in spec.rs).

### Increment 3: region.rs ✓
- [x] 9 items → stencil-region-api: Block enum (data-carrying variants), StencilFile struct, ParseError enum (whole-item); read_only/to_lines/parse/to_source/binding/slot_names (methods, incl. multi-line read_only signature). sync (9 created); compiles; 72 tests; second sync no-op (the rebuilt parser round-trips its own dogfooded source).

### Remaining modules
- [ ] engine.rs (9 items — SlotStatus/SlotOutcome/SyncReport/SyncOutcome + sync_file/kinograph_slot_names + SyncReport methods)
