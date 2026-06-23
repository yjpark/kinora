---
# kinora-3guj
title: 'Dogfood: extend stencil api-kinograph to engine/region/spec/kinds/lib'
status: todo
type: task
created_at: 2026-06-23T00:56:17Z
updated_at: 2026-06-23T00:56:17Z
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
