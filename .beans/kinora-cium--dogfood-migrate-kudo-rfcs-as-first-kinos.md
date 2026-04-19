---
# kinora-cium
title: 'Dogfood: migrate kudo RFCs as first kinos'
status: in-progress
type: task
priority: normal
created_at: 2026-04-18T09:16:59Z
updated_at: 2026-04-19T04:04:52Z
parent: kinora-w7w0
blocked_by:
    - kinora-860i
    - kinora-6zxd
    - kinora-zboo
    - kinora-9nom
---

Take kudo's RFC-0003 (and related RFCs) into `.kinora/store/` as the first kinos via `store`, create at least one kinograph composing them, verify the rendered output reads cleanly.

**Blocks RFC-0003 being marked done in kudo.**

RFC-0003 section: *Bootstrap Sequence* (steps 3–5).

## Acceptance

- [~] Each selected RFC is present as a kino with provenance recorded — RFC-0003 imported (id `e55f4908…`, lineage `7a155b58`); RFC-0001/0002 pending
- [ ] At least one kinograph composes related RFCs — deferred until ≥2 RFCs imported
- [x] `render` produces a readable mdbook site — verified end-to-end with `mdbook build`
- [ ] Any issues captured as new beans or updates to earlier beans


## Night-shift scope note

Deferred from autonomous overnight work — this step writes to the kudo repo (cross-repo), so it needs supervised execution. Move back to `todo` when ready to run manually.

## Plan

Dogfood RFC-0003 into this repo (kinora) as the first kino.

1. Ensure `.kinora/store/` and `.kinora/ledger/` exist (config.styx is already present).
2. `kinora store markdown --name rfc-0003 --author "YJ Park" --provenance "kudo:docs/src/rfcs/rfc-0003_kinora-bootstrap.md@<commit>" -m kind=rfc -m rfc-id=RFC-0003 < path/to/rfc-0003.md`
3. `kinora render --cache-dir /tmp/kinora-dogfood` and sanity-check with `mdbook build`.
4. Capture the kino id, commit `.kinora/` to git.
5. Defer kinograph composition until RFC-0001 and RFC-0002 are also imported — one-entry kinographs add no value.

Acceptance bounded to RFC-0003 only for this commit; follow-up stores for RFC-0001/0002 + a `kudo-rfcs` kinograph can be a second pass.
