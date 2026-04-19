---
# kinora-61f9
title: 'Phase 2B: `kinora compact` command'
status: in-progress
type: feature
priority: normal
created_at: 2026-04-19T06:50:57Z
updated_at: 2026-04-19T07:11:40Z
parent: kinora-xi21
blocked_by:
    - kinora-h4xs
---

Phase 2B of the kinora-xi21 architecture. Implements manual compaction: read all hot events, derive a single flat root kinograph version deterministically, store it, and update the root pointer file.

Blocked by: kinora-h4xs (`root` kind + entry schema).

## Design

### Library surface

`crates/kinora/src/compact.rs`:

```
pub struct CompactResult {
    pub root_name: String,
    pub new_version: Option<Hash>,   # None when no-op
    pub prior_version: Option<Hash>,
}

pub fn compact(
    kinora_root: &Path,
    root_name: &str,
) -> Result<CompactResult, CompactError>;
```

### Algorithm

1. Read the current root pointer at `.kinora/roots/<name>` (if any → prior_version).
2. Read all hot events via `Ledger::read_all_events()`.
3. For each distinct kino identity, pick the head version — the event that is not a parent of any other event for the same id. (No merges mid-compaction — assign events come in phase 3.)
4. Build a `Kinograph` of kind `root` with one flat entry per head kino:
   - `id`, `version`, `kind` taken from the head event
   - `metadata` = the head event's metadata (as-is)
   - `note`, `pin` absent in phase 2 (added by phase 3's assign events)
5. Sort entries by id; serialize via canonical `to_styx()`.
6. If the prior root's content bytes are byte-identical → no-op, return `new_version = None`.
7. Otherwise, `store_kino` with `parents = prior_version.iter().collect()`, `id = None` if no prior (genesis) else `id = prior_root.id`.
8. Write the new version's content hash to `.kinora/roots/<name>` (tmp+rename).
9. Return `CompactResult { new_version: Some(h), … }`.

### Determinism

- Entries sorted by id (ascii-hex)
- Metadata keys sorted (already canonical from BTreeMap)
- When post-merge compaction has two prior root versions (left+right), `parents` lists them in canonical hash order — phase-2B covers this path but the test can be small since we're primarily exercising the genesis + single-parent happy path

### Default root name

Phase 2 ships single-flat-root. Default `--root main` when not specified. Phase 3 generalizes.

### CLI

`kinora compact [--root <name>]`:
- On success, print `root=<name> version=<sh> (new version)` if promoted; `root=<name> version=<sh> (no-op)` otherwise.
- Use the same author/provenance/ts resolution pattern as `kinora store` (git author + RFC3339 ts).

## Acceptance

- [x] `compact()` library fn: genesis case (no prior root) produces a root with `parents[]` empty
- [x] Subsequent compaction: `parents = [prior_version]`, new `version` hash differs
- [x] Idempotence: `compact` with no new events is a no-op (`new_version = None`), pointer file unchanged
- [x] Two independent compactions over the same hot-event set produce byte-identical root blobs (cross-dev determinism test)
- [x] Entry order is sorted by id — parse output and assert
- [x] Pointer file `.kinora/roots/<name>` contains the 64-hex version hash only (no trailing whitespace/newline, or explicit trailing newline — pick one and test it)
- [x] CLI `kinora compact` prints expected output, exits 0
- [x] Integration test: store 3 markdown kinos → compact → assert root has 3 entries → store a v2 → compact again → assert root has 3 entries with one bumped version
- [x] Zero compiler warnings

## Out of scope

- `assign` events / moving kinos between roots (phase 3)
- Multi-root / per-root policy / config.styx root declarations (phase 3)
- GC/prune (phase 3)
- Git hooks (explicitly deferred)
- Sub-kinograph entries (phase 4)
