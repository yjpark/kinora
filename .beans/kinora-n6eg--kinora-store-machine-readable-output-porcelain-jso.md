---
# kinora-n6eg
title: 'kinora store: machine-readable output (--porcelain/--json)'
status: todo
type: task
priority: normal
created_at: 2026-06-26T00:48:24Z
updated_at: 2026-06-26T00:48:24Z
---

kinora store prints id=<hash> (with '='), easy to mis-parse vs the styxl 'id <hash>' (space) form; cost a set -e exit when scripting. Provide stable machine-readable output (--porcelain or --json) for store (and audit other commands for the same).
