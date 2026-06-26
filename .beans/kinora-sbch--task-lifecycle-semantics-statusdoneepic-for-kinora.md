---
# kinora-sbch
title: Task lifecycle semantics (status/done/epic) for kinora-as-tracker
status: draft
type: feature
priority: low
created_at: 2026-06-26T00:48:24Z
updated_at: 2026-06-26T04:15:45Z
---

Using kinora as a task tracker needs status/done/epic semantics; kinora has none. Currently proxied with prose notes in a plan kino plus the root GC policy. Note: overlaps with existing draft architecture epics; needs design before implementation. Filed from dogfooding so it is not lost.

## Design analysis (deferred — needs a design decision)

Reviewed during the findings night-shift; left as `draft`. Task-lifecycle
semantics (status/done/epic) are a substantial data-model addition that
overlaps the existing architecture epic [[kinora-xi21]] and the GC-waterline
proposal (kinora-1lp7) — they should be designed together, not bolted on.

Open questions a design must settle before implementation:
- Where does status live? Metadata (`status=todo|doing|done`, reusing the
  reserved-key mechanism) vs. a dedicated event kind vs. a root-per-status.
- How do "done" tasks sink? This is exactly the GC-waterline question
  (kinora-1lp7): done tasks → short-term root (maxage/keep-last-N), specs →
  durable (never).
- Epic/parent relationships likely reuse the typed-link primitive (ysfb) —
  blocked on that.

Recommendation: design kinora-sbch + kinora-1lp7 + kinora-ysfb together as one
"agentic tracking" design pass under [[kinora-xi21]].
