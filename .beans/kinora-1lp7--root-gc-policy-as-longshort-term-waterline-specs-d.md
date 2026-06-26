---
# kinora-1lp7
title: Root GC policy as long/short-term waterline (specs durable, tasks sink)
status: draft
type: feature
priority: deferred
created_at: 2026-06-26T00:48:24Z
updated_at: 2026-06-26T00:48:24Z
---

Root GC policy modeled as a waterline: specs = never (durable, above water); tasks = maxage/keep-last-N (short-term, sinks after done). A nice fit observed during dogfooding, deferred until the broader policy design settles. Relates to kinora-xi21 (architecture) and the task-lifecycle feature.
