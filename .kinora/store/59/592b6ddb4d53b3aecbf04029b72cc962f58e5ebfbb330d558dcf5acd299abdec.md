# Spec versions: graduation labels + pin-all-entries

A proposed resolution (not yet committed) to the RFC open question of how a spec
version — "this api-kinograph version = crate foo v0.4.0" — is declared.

## The decision (proposed)
- **Open between three mechanisms:** a named root per crate vs a
  `kudo::spec-version` kino vs a ledger event. This bean exists to resolve which.
- **Proposed: graduation pins all api-kinograph entries to content hashes,
  reusing `Entry::pin`** (kino://5898fe2453ef8824f71670df106512d00502edbc90ac0fa8705a5d03e6f98c25 established that pinning rides the
  existing `Entry::pin` — no new mechanism). A spec version is the act of
  pinning every entry; before graduation, entries follow head.
- **Decoupled:** spec versioning is independent of kinograph evolution and of
  semver — graduating a version is an explicit pin-all, not an automatic
  consequence of editing the kinograph.
- **Not a dead end:** the three declaration mechanisms are still open; the
  pin-all-via-`Entry::pin` semantics are the committed part, the version-label
  storage is the part to settle.

## What this would build
- A graduation command/flow that pins every api-kinograph entry and records the
  version label, plus the chosen storage for the label.

## Consequences / follow-ups
- Parent architecture: kino://5898fe2453ef8824f71670df106512d00502edbc90ac0fa8705a5d03e6f98c25; reuses the `Entry::pin` decision there.
- Interacts with drift enforcement: kino://60c87365c3ed86c8c8472237d32a7827699354e042b4821673efdef0af28996c.
