---
# kinora-q28s
title: 'Spec kino content model: markdown + fenced code split'
status: todo
type: feature
priority: high
created_at: 2026-06-06T09:28:31Z
updated_at: 2026-06-06T09:28:42Z
parent: kinora-bm7z
blocked_by:
    - kinora-vsuo
---

Parse a kudo::api-spec blob into a SpecItem { doc_prose, code_fragments } using pulldown-cmark. Prose before/around fenced blocks becomes the doc contract; rust fenced blocks become the signature code (concatenated, in order). Handle multiple blocks, no-prose, no-code edge cases. TDD.
