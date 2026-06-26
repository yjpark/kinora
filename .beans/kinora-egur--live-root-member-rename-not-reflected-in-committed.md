---
# kinora-egur
title: Live root-member rename not reflected in committed root kinograph
status: todo
type: bug
priority: normal
created_at: 2026-06-26T00:48:23Z
updated_at: 2026-06-26T00:48:23Z
---

A metadata-only rename (store same content with --id, no --parents) updates the ledger head but does NOT bump the root version, so the committed root kinograph keeps the old member name. Consumers must read the live head name from the ledger. The committed root should reflect the live name, or this should be documented as intended with a render-side guidance.
