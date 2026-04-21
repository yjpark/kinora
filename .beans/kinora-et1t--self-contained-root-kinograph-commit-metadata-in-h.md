---
# kinora-et1t
title: 'Self-contained root kinograph: commit metadata in header line'
status: completed
type: feature
priority: normal
created_at: 2026-04-21T15:21:11Z
updated_at: 2026-04-21T16:32:45Z
---

## Context

After `kinora commit` + `kinora repack`, `.kinora/staged/` still holds one `kind: root` store event per named root — each carries the root lineage's `id`, `parents`, `ts`, `author`, `provenance`. These are durable **commit metadata**, not pending work. They live in staging only because `store_kino` unconditionally writes events there.

This makes repack awkward (those roots can never be drained without breaking reformat's `PriorRootEventMissing` lookup and losing the version chain) and violates the mental model: "staged = new events, not yet promoted."

## Decision

Inline the commit metadata as a **header line at the top of the root styxl blob**. Root kinographs then self-contain the full commit chain. No separate root store event is written at all.

After commit + repack, the repo layout is exactly:

```
.kinora/
  config.styx
  roots/<name>          # pointer → current root version hash
  store/<ab>/<hash>.styxl   # root blob contents
  store/<ab>/<hash>.md      # kino content blobs
```

`.kinora/staged/` is only populated with *pending* events (new store + assign events since last commit). Empty after repack.

## Format

Styxl root blob — line 1 is a `RootHeader`, lines 2+ are `RootEntry` (unchanged shape):

```
{kind root, id <root-lineage-id>, parents [<prior>, ...], ts <rfc3339>, author "YJ", provenance "commit"}
{id <entry-id>, version <content-hash>, kind markdown, metadata {...}, note "", pin false, head_ts <ts>}
{id <entry-id>, ...}
...
```

Disambiguation: header line has `kind root` at top level + `parents [...]`. Entry lines have `version` + `head_ts`. No ambiguity.

Genesis version → `parents []` (empty list).

## Scope

- [x] New `RootHeader` struct in `root.rs` (id, parents, ts, author, provenance). `RootKinograph` gains a `header: RootHeader` field.
- [x] `to_styxl` writes header line first, then sorted entries. `parse_styxl` reads line 1 as header, 2+ as entries.
- [x] Hard cutover: new format is the ONLY root format. Styx-wrapped `entries (…)` and header-less styxl no longer parse. Existing repos must nuke `.kinora/` and re-store (history not preserved — kinora is pre-1.0).
- [x] `commit_root_with_refs` builds the `RootHeader` from the commit's author/provenance/ts and sets `parents` from the prior pointer (if any). Does NOT call `store_kino` with `kind: root` anymore — writes the content blob + the `roots/<name>` pointer directly, no staged event.
- [x] `reformat.rs` — drop the `PriorRootEventMissing` lookup and the root-rewrite path entirely. Reformat's Step 1 (rewriting legacy root blobs) no longer exists; the hard cutover means there are no legacy roots. Step 2 (kinograph-kind reformat) stays — those are still relevant for styx→styxl content migration.
- [x] `ledger` / `clone` / `repack` — no longer special-case root events; staged/ is purely content + assign events.
- [x] Remove `PriorRootEventMissing` error (no longer reachable).
- [x] CLI `kinora store list` / similar that display `kind: root` events: continue to work off the root blob, not ledger events.

## Migration

No migration code. Kinora is pre-1.0. Existing repos whose roots predate this change won't parse. Users upgrade by `rm -rf .kinora/ && kinora init && <re-stage content>`. History before cutover isn't preserved; acceptable since no repo has content we can't reconstruct from source.

## Tests

- [x] `root.rs`: roundtrip header + entries; empty-parents (genesis) roundtrip; header-with-multiple-parents roundtrip. (Parse-rejection tests removed: facet_styx tolerates unknown-field shapes; hard cutover is enforced at the write side instead.)
- [x] `commit.rs`: `commit_all` on a fresh repo produces a root blob whose header has `parents []` and correct ts/author/provenance; no staged root event is written.
- [x] `commit.rs`: second commit's root has `parents [first_root_hash]`.
- [x] `repack.rs`: after commit + repack on a fresh repo, `.kinora/staged/` is empty.
- [x] `reformat.rs`: existing nested-pin, idempotence, already-styxl kinograph tests still pass (adjusted for the fact that root blobs now always carry a header).

## Acceptance

- [x] All tests pass, zero warnings.
- [x] Round-trip: `init → store_md → commit → repack` → `staged/` is empty; roots/ and store/ are populated; no staged store events.
- [x] Walking version history: read `roots/<name>` → read blob → parse header → recurse `parents`. No ledger read required.
- [x] `kinora render` / `kinora resolve` still work unchanged against the new format.

## Out of scope (defer to follow-ups)

- Merging `store_kino` and `commit_root_with_refs` into a unified "make a new kino version" path — the split exists for reasons beyond this change.
- Cross-repo federation or signed commits (author field is just a string for now).

## Clarifications (from 2026-04-21 discussion)

- **Stable lineage id**: the header's `id` is stable across all versions of a named root (mirrors kino identity — a kino's id is its genesis content hash, immutable through the version chain). A root's lineage id is assigned on genesis and carried through every subsequent header. Concretely: `inbox`'s id on commit #1 is the hash of the genesis blob; commit #2's header carries the same `id` with `parents [commit1_hash]`.
- **No extra header fields**: id, parents, ts, author, provenance only. No policy snapshot, head count, or anything else. YAGNI; add later if needed.
- **Hard cutover**: confirmed. No legacy-parse, no reformat migration path. Upgrade = nuke + re-stage.

## Open implementation question

How is the lineage id generated on genesis? Two options:

- **A**: Hash the genesis root blob's entries payload (without the header) and use that as id. Deterministic but chicken-and-egg: header.id depends on a hash that excludes the header.
- **B**: Derive from the root name deterministically (e.g., `blake3("root:" + name)`). Independent of content; two repos with the same root name get the same lineage id.

**Resolved: A.** Genesis header's `id` = hash of entries-only bytes (excluding header line). Subsequent headers carry that id forward unchanged. Parser treats id as informational (doesn't re-verify against content).

## Summary of Changes

- `RootHeader { kind: "root", id, parents, ts, author, provenance }` is now line 1 of every root styxl blob. `kind` is the parse discriminator that keeps entry lines from being read as headers on legacy input.
- `RootKinograph::new_genesis` / `new_child` are the canonical constructors for new root versions. Genesis derives the lineage id from `hash(entries-only canonical bytes)`; `new_child` carries that id forward and records prior blob hashes in `parents`.
- `commit_root_with_refs` builds the header from the commit's author/provenance/ts, writes the blob via the content store, and updates the `roots/<name>` pointer directly. No `kind:root` staged event is written — `.kinora/staged/` is now purely pending content/assign events. Post-commit+repack, `staged/` is empty (pinned by a new `repack.rs` test).
- Hard cutover: legacy styx-wrapped and header-less styxl root blobs no longer parse. `reformat.rs` Step 1 (legacy root rewrite) and the `PriorRootEventMissing` error are both gone. Step 2 (kinograph reformat) stays — that's orthogonal content migration.
- CLI `kinora reformat` summary no longer reports root counts. CLI doc string updated.
- Tests: header roundtrip, genesis/child lineage, multi-parent headers, 3-version lineage-id stability, entries-unchanged no-op (pre-existing), empty-genesis no-op (pre-existing), repack zero-staged-events, and `WrongHeaderKind` rejection on header-less blobs.

### Commits

- `4e80bf5` refactor(root): inline commit metadata in root kinograph header
- `b64f6cf` test(repack): assert staged/ is empty after commit+repack
- `7e79803` fix(root): enforce header discriminator; cover lineage+cutover edge cases

### Follow-up

Users of pre-et1t repos (including this repo's own `.kinora/`) must nuke and rebuild: `rm -rf .kinora/ && kinora init && <re-store>`. No migration code.
