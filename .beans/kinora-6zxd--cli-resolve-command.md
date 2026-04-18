---
# kinora-6zxd
title: 'CLI: resolve command'
status: in-progress
type: feature
priority: normal
created_at: 2026-04-18T09:16:59Z
updated_at: 2026-04-18T16:22:38Z
parent: kinora-w7w0
blocked_by:
    - kinora-5k13
---

Given a kino name or id, returns its current content. Supports fork detection and version selection.

RFC-0003 section: *Minimal CLI → resolve*. Design decisions in `kinora-fhw1`.

## Design

### Command shape

```
kinora resolve <name-or-id> [--version HASH] [--all-heads]
```

### Lookup algorithm

1. Scan all lineage files under `.kinora/ledger/`
2. Collect events whose `id` matches (direct lookup) or whose metadata `name` matches (name lookup; warn on ambiguity)
3. Filter to events belonging to this identity (same `id`)
4. Build version DAG: for each event, filter `parents[]` to those of the same identity
5. Find heads (events not referenced as parent by any later same-identity event)
6. Single head → return its content
7. Multiple heads:
   - Branch-aware: if HEAD's lineage descends from one head unambiguously, return that
   - Otherwise: refuse with actionable report

### Fork report shape

```
kino `content-addressing` (id: b3aaa…) has 2 heads:
  - b3xxx… (lineage <shorthash>, yj @ 2026-04-10)
  - b3yyy… (lineage <shorthash>, yj @ 2026-04-12)

Reconcile via one of:
  - merge:     kinora store <kind> --id b3aaa… --parents b3xxx…,b3yyy… <content>
  - linearize: pick one head; write as new version with both as parents
  - keep-both: append metadata event introducing variant names
  - detach:    treat one head as new identity
```

## Acceptance

- [x] `resolve <id>` returns content of current head
- [x] `resolve <name>` does name lookup via metadata; errors on ambiguity
- [x] Fork detection traverses version DAG within identity
- [x] Single head → return content
- [x] Multiple heads → refuse with actionable report (heads listed, reconcile commands shown)
- [x] Branch-aware resolution: head in HEAD lineage wins tiebreak
- [x] `--version HASH` returns specific prior version's content
- [x] `--all-heads` flag returns all heads without erroring
- [x] Unknown name/id yields clear error

## Plan

Library-first again — resolution logic belongs in `kinora` so it's reusable (kinograph-zboo needs the same name→id resolution on store).

**`crates/kinora/src/resolve.rs` (new):**

- `Identity { id, events, heads, lineages }` — all events for a single id, with heads computed as leaves of the within-identity version DAG.
- `Resolver` — loads all identities up-front from `Ledger::read_all_lineages`. Methods:
  - `resolve_by_id(id) -> Result<Resolved, ResolveError>` — exact 64-hex match; errors on unknown.
  - `resolve_by_name(name) -> Result<Resolved, ResolveError>` — scan metadata["name"] across latest version of each identity; errors on 0 or >1 hit.
  - `resolve_at_version(id, hash) -> Result<Resolved, ResolveError>` — specific version lookup.
- `Resolved { id, head: Event, content: Vec<u8>, lineage, all_heads }` — returns the chosen head plus content bytes (read + hash-verified via ContentStore).
- `ResolveError` variants: NotFound, AmbiguousName { name, ids }, MultipleHeads { id, heads, lineages }, Store/Ledger/Hash parse wrapping.

Branch-aware resolution: if multiple heads but HEAD-lineage contains exactly one of them, prefer that head. Otherwise surface MultipleHeads.

**CLI (`crates/kinora-cli/src/`):**

- Add `Command::Resolve` to cli.rs (positional `name_or_id`, optional `--version HASH`, `--all-heads` flag).
- `resolve.rs` — format output:
  - success: write content to stdout
  - fork: pretty-print actionable report matching the bean spec (heads with timestamps, suggested reconcile commands)
- Wire into main.rs dispatch.

Commit plan:
1. resolve library + tests
2. resolve CLI + tests
3. review fixes if any
