---
# kinora-ysfb
title: Typed non-composition links between kinos + reverse lookup
status: draft
type: feature
priority: normal
created_at: 2026-06-26T00:48:24Z
updated_at: 2026-06-26T04:15:33Z
---

The only structural link between kinos is the kinograph (an ordered composition, meant for rendering/inlining). There is no primitive for a plain typed reference — task->plan, task->spec, code->spec. Today these are encoded as kino-id references in prose/comments, with no reverse lookup ('what references this kino?'). Desired: a typed, non-composition link primitive with reverse traversal.

## Design analysis (deferred — needs a design decision)

Investigated during the findings night-shift. Not implemented: a typed link
primitive is a data-model addition whose shape (storage, validation, GC
interaction, query surface) warrants a deliberate design decision rather than
an overnight guess. Captured here so it's ready to pick up.

### Current state
- A `links` bare metadata key is ALREADY reserved (`namespace.rs`
  `RESERVED_METADATA_KEYS`) but has no defined format or semantics yet.
- Kinos can already reference each other untyped via `kino://<id>` URLs in
  content; `render::rewrite_kino_urls` resolves those to page links. There is
  no reverse index.

### Recommended design
1. Define the `links` metadata value as a compact typed list, e.g.
   `spec:<id>,plan:<id>` (type token + target id). Validate that each target
   is a 64-hex id (or a name resolved at store time, like kinograph entries).
2. Key decision — links are NON-composition: unlike kinograph pins they must
   NOT pin/protect their targets from GC (the bean explicitly calls them
   "non-composition"). Confirm this so the resolver/commit GC ignores them.
3. Reverse lookup: add `Resolver::referrers(id) -> Vec<(referrer_id, type)>`
   scanning every identity head's `links` metadata. O(n) over identities is
   fine at current scale; an index can come later.
4. CLI surface: `kinora links <id>` (outgoing) and `kinora backlinks <id>`
   (incoming), honoring `--json` (see kinora-n6eg).

### Safe first increment (could ship independently)
A read-only reverse index over existing `kino://<id>` content references —
delivers "what references this kino?" with zero schema commitment. Worth a
follow-up bean if the typed-link design stalls.
