---
# kinora-thow
title: Marker protocol + region parser + LanguageTarget trait
status: completed
type: feature
priority: high
created_at: 2026-06-06T09:28:31Z
updated_at: 2026-06-06T10:41:51Z
parent: kinora-bm7z
blocked_by:
    - kinora-vsuo
---

Line-based parser for stencil:kinograph / stencil:slot / stencil:ro..stencil:end. Model a target file as an ordered sequence of editable text + slots + read-only blocks; round-trip (parse -> serialize -> parse) is stable and preserves all non-stencil bytes. Define LanguageTarget (comment leader, doc-comment formatting, read-only block emission); implement RustTarget. TDD.

## Todo

- [x] `LanguageTarget` trait (comment leader, doc-comment formatting) + `RustTarget`
- [x] `Block` model + `StencilFile` (text / binding / slot / read-only)
- [x] Line-based marker parser (kinograph / slot / ro..end), error cases
- [x] Read-only block emission + byte-stable round-trip serialize
- [x] Tests: round-trip, indentation, error cases, doc formatting
- [x] Wire `ParseError` into `StencilError`; zero warnings; code review

## Summary of Changes

Added the marker protocol, region model, and language-target abstraction to the stencil lib.

- **`target.rs`** — `LanguageTarget` trait (comment leader + doc-comment formatting) and `RustTarget` (`//` markers, `///` docs).
- **`region.rs`** — `Block` (Text / Binding / Slot / ReadOnly) + `StencilFile`. Line-based parser recognizing `stencil:kinograph` / `stencil:slot` / `stencil:ro`…`stencil:end`, capturing per-marker indentation; `to_source` re-emits canonically. `parse → to_source` round-trips stencil-written input byte-for-byte (editable Text preserved verbatim; markers normalized — stencil owns them). `ParseError` covers unterminated/unexpected-end/nested-ro/malformed, with 1-based line numbers. Helpers: `binding()`, `slot_names()`, `Block::read_only()`, `Block::to_lines()`.
- **`lib.rs`** — exposed `region`/`target` modules; wired `ParseError` into `StencilError`.

35 tests (round-trip incl. empty/blank/trailing-newline/indented/mixed files, all error paths, doc formatting, deliberate marker normalization). Zero warnings. Code-reviewed by a fresh-perspective subagent — no correctness issues; the two normalization asymmetries it flagged are intended and now pinned by tests.
