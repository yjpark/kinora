---
# kinora-ysfb
title: Typed non-composition links between kinos + reverse lookup
status: todo
type: feature
priority: normal
created_at: 2026-06-26T00:48:24Z
updated_at: 2026-06-26T00:48:24Z
---

The only structural link between kinos is the kinograph (an ordered composition, meant for rendering/inlining). There is no primitive for a plain typed reference — task->plan, task->spec, code->spec. Today these are encoded as kino-id references in prose/comments, with no reverse lookup ('what references this kino?'). Desired: a typed, non-composition link primitive with reverse traversal.
