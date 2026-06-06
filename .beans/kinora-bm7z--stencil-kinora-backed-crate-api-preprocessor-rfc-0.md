---
# kinora-bm7z
title: 'Stencil: kinora-backed crate API preprocessor (RFC-0004)'
status: in-progress
type: epic
priority: high
created_at: 2026-06-06T09:27:42Z
updated_at: 2026-06-06T09:28:17Z
---

Implement RFC-0004: stencil, a silp-inspired, kinora-native, language-agnostic preprocessor that renders a crate's API spec (held as kinora kinos) into read-only sections of source files, preserving editable regions.

## Source

Implements [RFC-0004](~/edger/kudo/docs/src/rfcs/rfc-0004_kinora-backed-crate-apis.md)
in the kinora repo. Stencil is silp-inspired (mixing model), kinora-native
(inputs), language-agnostic (design), Rust-first (bootstrap target).

## Resolved design decisions (planning session 2026-06-06)

1. **Packaging** — separate crates in this workspace: `crates/stencil` (lib,
   depends on the `kinora` crate) + `crates/stencil-cli` (bin `stencil`).
   Mirrors the kinora/kinora-cli split. kinora itself needs no changes —
   `kudo::api-spec` / `kudo::api-kinograph` are already valid namespaced kinds.

2. **Spec kino content = markdown + fenced code.** A `kudo::api-spec` kino is a
   markdown blob: prose behavioral contract followed by one or more ```rust
   fenced blocks. Stencil splits it (via `pulldown-cmark`, already a kinora dep):
   prose → `///` doc-comments, fenced code → the signature body. Renders cleanly
   in mdbook too. Language-agnosticism lives in the pluggable target, not the
   kino format.

3. **Region model = in-place, agent-placed slots.** The agent drops a
   `// stencil:slot <entry-name>` marker wherever it wants the item; stencil
   fills/refreshes only its own read-only block at that slot and never touches
   anything else. Maximum layout freedom; honors "stencil is the sole author of
   read-only sections".

4. **First slice = engine + dogfood stencil.** Build the render/sync engine,
   then define stencil's own public API as a kinograph and rebuild stencil's
   source from it (RFC day-one dogfood). Defer spec-versioning, hybrid
   test-scaffolds, reverse `extract`, extra language targets, drift linter, and
   a real pilot crate to follow-up beans.

## Reconciliation: sync is kinograph-bound

The in-place model is file-centric, but "API as kinograph" makes the kinograph
the source of truth for membership + pinning. Reconciliation:

- A target file declares its binding once: `// stencil:kinograph <name|id>`.
- `stencil sync <paths>` loads that api-kinograph (reusing kinora's `Kinograph`
  struct — the api-kinograph blob is the same `{id,name?,pin?,note?}` styxl
  entry shape, just `kind: kudo::api-kinograph`), then for each
  `// stencil:slot <entry-name>` in the file, matches the entry **by name**,
  resolves it (entry `pin` if set, else head, via kinora `Resolver`), splits the
  spec kino, and writes the read-only block.
- Slots with no matching entry → error. Entries with no slot → warning (and
  `scaffold` can emit them).
- Pinning rides the existing `Entry::pin` — no new mechanism. Follow-head during
  design; pin-all is what a future "spec version" graduation does.

## Marker protocol (RFC open question — concrete proposal; spelling refinable)

Line-based, no language parsing (silp ethos). Comment leader comes from the
language target (`//` for Rust):

```
// stencil:kinograph <name|id>        # file-level binding, once per file
// stencil:slot <entry-name>          # agent-placed anchor
// stencil:ro <entry-name> <hash>     # stencil-written; opens read-only block
<rendered /// doc-comments + signature code>
// stencil:end                        # closes read-only block
```

- The `slot` line is the durable, agent-controlled anchor. On sync, stencil
  (re)writes the immediately-following `ro … end` block; if absent it creates
  one, if present it refreshes it.
- The `<hash>` in the `ro` marker is the resolved content hash: enables no-op
  detection on unchanged re-runs and a "read-only region was hand-edited"
  warning (cheap precursor to the RFC's hash-checked enforcement).

## Language target trait

`LanguageTarget`: comment leader, how to format a doc-comment block, how to emit
a read-only block. Ships `RustTarget`. Other languages are additive (TS/Python
follow-ups) with no engine redesign — satisfies RFC principle 3.

## Deferred (follow-up beans, NOT in slice 1)

- Spec versions / graduation labels (RFC open Q: named root per crate vs
  `kudo::spec-version` kino vs ledger event). Pin-all-entries semantics.
- Hybrid test scaffolding: doctests → named `#[test] todo!()` scaffolds →
  prose-only.
- Reverse `stencil extract`: read-only region edit → new spec kino version.
- Additional language target (validates the target trait).
- Public-API drift linter / hash-checked read-only enforcement.
- Pilot a real published crate (RFC bootstrap step 4; open Q on where the
  kinora-extracted shared crate lives).

## No kinora-crate changes required

`kudo::api-spec` + `kudo::api-kinograph` pass `validate_kind` (namespaced).
api-kinograph content parses via `kinora::kinograph::Kinograph`. Resolution uses
`kinora::resolve::Resolver` (stencil adds a thin kind-scoped name lookup over
`Resolver::identities()`). Stencil is a new *renderer* with a source-file target;
it does not touch kinora's mdbook renderer.
