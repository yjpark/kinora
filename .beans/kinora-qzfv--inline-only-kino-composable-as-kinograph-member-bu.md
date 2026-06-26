---
# kinora-qzfv
title: 'inline-only kino: composable as kinograph member but never routed to its own page'
status: todo
type: feature
priority: normal
created_at: 2026-06-26T00:48:24Z
updated_at: 2026-06-26T00:48:24Z
---

Composing a post as a kinograph needs post-specific connective prose (intro/outro) that belongs to that post only. That prose must be a kino to sit in the kinograph, but it should appear ONLY inlined — never as a standalone page. Today every committed kino is swept into a root and every rooted markdown kino renders to its own page, so connective prose leaks out as orphaned fragment pages. Desired: a kino-level inline-only signal — a dedicated kind or a metadata flag (e.g. inline=true) meaning 'resolvable and composable as a kinograph member, but never routed to a standalone page.' Render tool inlines it into composing kinographs and skips emitting its own chapter.
