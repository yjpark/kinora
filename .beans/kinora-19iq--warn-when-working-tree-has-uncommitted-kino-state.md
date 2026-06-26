---
# kinora-19iq
title: Warn when working tree has uncommitted kino state at render time
status: todo
type: task
priority: normal
created_at: 2026-06-26T00:48:24Z
updated_at: 2026-06-26T00:48:24Z
---

render reads git-committed .kinora, not the working tree. Workflow gotcha: the loop must be store -> kinora commit -> git commit -> render. Add a kinora-side warning when the working tree has uncommitted kino state (staged events not yet git-committed) so the stale-render footgun is visible.
