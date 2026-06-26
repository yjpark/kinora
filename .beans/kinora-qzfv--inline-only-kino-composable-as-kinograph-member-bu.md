---
# kinora-qzfv
title: 'inline-only kino: composable as kinograph member but never routed to its own page'
status: completed
type: feature
priority: normal
created_at: 2026-06-26T00:48:24Z
updated_at: 2026-06-26T03:56:25Z
---

Composing a post as a kinograph needs post-specific connective prose (intro/outro) that belongs to that post only. That prose must be a kino to sit in the kinograph, but it should appear ONLY inlined — never as a standalone page. Today every committed kino is swept into a root and every rooted markdown kino renders to its own page, so connective prose leaks out as orphaned fragment pages. Desired: a kino-level inline-only signal — a dedicated kind or a metadata flag (e.g. inline=true) meaning 'resolvable and composable as a kinograph member, but never routed to a standalone page.' Render tool inlines it into composing kinographs and skips emitting its own chapter.

## Summary of Changes

Added an `inline=true` metadata signal so a kino can be composed as a
kinograph member without leaking out as its own standalone render page.

- `crates/kinora/src/namespace.rs`: registered `inline` as a reserved bare
  metadata key.
- `crates/kinora/src/render.rs`: `render()` skips page emission for any kino
  whose head metadata has `inline=true`. Only the exact string "true"
  suppresses the page. Inlining is unaffected: `Kinograph::render` resolves
  members by id independently of page emission, so inline-only members still
  inline into composing kinographs.

Tests: inline kino not emitted as a page; inline-only member still inlines
into a composing kinograph while a normal sibling keeps its page;
`inline=false` (and other values) still render as a page. Full workspace suite
green, zero warnings.
