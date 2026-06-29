# Task: Spec kino content model — markdown + fenced code split

**Goal.** Parse a `kudo::api-spec` blob into `SpecItem { doc_prose,
code_fragments }` using pulldown-cmark, splitting prose contract from ```rust
fenced blocks. TDD.

**Realizes:**
- kino://9c9c85472b2231290285d3dd7231fbac6b53d80a80e4c6c065a9cad5f8c67f96 — Spec kino content model: markdown prose + fenced code.

**Done when:** `parse` splits prose vs rust fenced blocks (offset-based
excision), handling multiple blocks, no-prose, no-code, non-rust fences,
attributes, and indented code; `from_bytes` decodes UTF-8 first; `SpecError` is
wired into `StencilError`; all tests pass with zero warnings.

## Outcome
Added the `spec` module: `SpecItem { doc_prose, code_fragments }`,
`parse(&str)` (infallible), `from_bytes(&[u8])` (`SpecError::NotUtf8`), `code()`,
`has_code()`. Offset-based split via `into_offset_iter`; seam-local prose
normalization preserves non-rust block interiors byte-for-byte. 50 tests; zero
warnings; fresh-perspective review fixed the one flagged normalization bug. This
unblocked the engine (kino://440900dd1b15bd83c02e26e87e0172cd1d43789bea58a632fb27ae9da70661a6) — both its inputs (region + spec
models) are in place.
