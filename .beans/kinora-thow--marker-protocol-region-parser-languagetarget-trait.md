---
# kinora-thow
title: Marker protocol + region parser + LanguageTarget trait
status: todo
type: feature
priority: high
created_at: 2026-06-06T09:28:31Z
updated_at: 2026-06-06T09:28:43Z
parent: kinora-bm7z
blocked_by:
    - kinora-vsuo
---

Line-based parser for stencil:kinograph / stencil:slot / stencil:ro..stencil:end. Model a target file as an ordered sequence of editable text + slots + read-only blocks; round-trip (parse -> serialize -> parse) is stable and preserves all non-stencil bytes. Define LanguageTarget (comment leader, doc-comment formatting, read-only block emission); implement RustTarget. TDD.
