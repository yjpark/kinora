---
# kinora-q28s
title: 'Spec kino content model: markdown + fenced code split'
status: completed
type: feature
priority: high
created_at: 2026-06-06T09:28:31Z
updated_at: 2026-06-10T00:07:33Z
parent: kinora-bm7z
blocked_by:
    - kinora-vsuo
---

Parse a kudo::api-spec blob into a SpecItem { doc_prose, code_fragments } using pulldown-cmark. Prose before/around fenced blocks becomes the doc contract; rust fenced blocks become the signature code (concatenated, in order). Handle multiple blocks, no-prose, no-code edge cases. TDD.

## Todo

- [x] Add `pulldown-cmark` dep; `spec` module with `SpecItem { doc_prose, code_fragments }`
- [x] Split markdown: prose contract vs ```rust fenced blocks (offset-based excision)
- [x] Edge cases: multiple blocks, no-prose, no-code, non-rust fences, attributes, indented code
- [x] `from_bytes` (UTF-8) + wire into `StencilError`
- [x] Tests + zero warnings + code review

## Summary of Changes

Added the `spec` module modelling a `kudo::api-spec` kino's content.

- **`SpecItem { doc_prose, code_fragments }`** — `parse(&str)` (infallible) splits markdown into the prose contract and the inner text of each ```rust fenced block (in order). `from_bytes(&[u8])` decodes UTF-8 first (`SpecError::NotUtf8`, wired into `StencilError`). Convenience: `code()` (fragments joined by a blank line), `has_code()`.
- **Offset-based split** — pulldown-cmark `into_offset_iter` locates the rust code blocks; their byte ranges are excised from the prose, their inner text collected as fragments. `info_is_rust` accepts `rust` / `rust,ignore` / etc. Non-rust fences and indented code stay in the prose.
- **Seam-local prose normalization** — blank lines are collapsed only at excision seams, so surviving non-rust block interiors are preserved byte-for-byte (caught in code review; pinned by a test).

50 tests (prose/code permutations, multiple/zero blocks, attributes, non-rust + indented blocks, multiline signatures, UTF-8, markdown-formatting + verbatim preservation). Zero warnings. Fresh-perspective code review: offsets/excision/API-usage sound; the one flagged normalization bug is fixed.

This unblocks the engine (kinora-hgpl) — both its inputs (region model + spec model) are now in place.
