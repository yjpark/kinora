---
# kinora-1lp7
title: Root GC policy as long/short-term waterline (specs durable, tasks sink)
status: draft
type: feature
priority: deferred
created_at: 2026-06-26T00:48:24Z
updated_at: 2026-06-26T04:15:45Z
---

Root GC policy modeled as a waterline: specs = never (durable, above water); tasks = maxage/keep-last-N (short-term, sinks after done). A nice fit observed during dogfooding, deferred until the broader policy design settles. Relates to kinora-xi21 (architecture) and the task-lifecycle feature.

## Design analysis (deferred — needs a design decision)

Reviewed during the findings night-shift; left as `draft`/`deferred` per the
original filing ("deferred until the broader policy design settles"). The
waterline model (specs=never durable; tasks=maxage/keep-last-N short-term)
is sound but is a policy redesign that should be designed alongside the
task-lifecycle work (kinora-sbch) under the architecture epic [[kinora-xi21]].

The GC machinery to support it largely exists already (RootPolicy::Never /
MaxAge / KeepLastN, head_ts-based entry GC). The open design work is the
user-facing model and config surface, not the mechanism.
