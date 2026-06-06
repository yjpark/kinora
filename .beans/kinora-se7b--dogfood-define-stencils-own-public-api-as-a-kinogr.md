---
# kinora-se7b
title: 'Dogfood: define stencil''s own public API as a kinograph and rebuild stencil src from it'
status: todo
type: task
priority: high
created_at: 2026-06-06T09:28:42Z
updated_at: 2026-06-06T09:28:43Z
parent: kinora-bm7z
blocked_by:
    - kinora-exay
    - kinora-guv8
---

Author kudo::api-spec kinos for stencil's public surface (markdown+fenced-rust); compose a kudo::api-kinograph; place stencil:kinograph + stencil:slot markers in stencil's source; run stencil sync; verify the read-only blocks match, the crate still compiles, and a second sync is a no-op. RFC day-one dogfood. Blocked by the engine + CLI.
