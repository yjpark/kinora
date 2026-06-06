---
# kinora-exay
title: 'CLI: stencil sync <paths>'
status: todo
type: feature
priority: high
created_at: 2026-06-06T09:28:42Z
updated_at: 2026-06-06T09:28:43Z
parent: kinora-bm7z
blocked_by:
    - kinora-hgpl
---

Scan given paths (files or dirs, recursive; default cwd) for stencil markers, apply the engine, and write changed files atomically. Report: files changed, slots filled/refreshed, drift warnings, entries with no slot. Non-zero exit on errors (unknown slot, parse failure). TDD/integration.
