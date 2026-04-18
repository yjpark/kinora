---
# kinora-zboo
title: Kinograph composition format
status: in-progress
type: feature
priority: normal
created_at: 2026-04-18T09:16:59Z
updated_at: 2026-04-18T16:27:33Z
parent: kinora-w7w0
blocked_by:
    - kinora-5k13
---

Kinograph composition — `kind: kinograph` with styx `entries[]` content. Each entry references a kino by identity.

RFC-0003 section: *Kinographs*. Design decisions in `kinora-fhw1`.

## Design

### Content format (styx)

```styx
entries:
  - id: b3aaa…
  - id: b3bbb…
    name: content-addressing
    pin: b3xxx…
    note: "The atomic concept — everything else builds on this."
  - id: b3ccc…
```

- `id` (required): authoritative kino-id reference
- `name` (optional): non-authoritative hint; renderer warns if current name differs
- `pin` (optional): freeze this reference to a specific content hash
- `note` (optional): short commentary about this composition choice

### Metadata on ledger event

- `title` — human title
- `description` — longer prose describing the composition
- `entry_notes` — optional per-entry notes keyed by kino id
- Namespaced extensions allowed

### Authoring flow

1. User writes kinograph source with names or ids
2. `kinora store kinograph <path>` resolves names to ids against current ledger state
3. Stored content has ids filled in (authoritative); name hints preserved
4. Raw-file readability preserved
5. Renaming a referenced kino later does not break the kinograph (id is stable)

### Rendering

- Walk `entries[]` in order
- For each entry: resolve `id` (or pinned hash); fetch kino content; inline
- If referenced kino is itself a kinograph: recurse (stretch goal) or warn
- Optional per-entry notes rendered as leading blockquote

## Acceptance

- [ ] Parses styx kinograph with `entries[]`
- [ ] Entry shape validated: `{id, name?, pin?, note?}`
- [ ] Name→id resolution on store (warn on ambiguous or missing)
- [ ] Pinned refs resolve to specific content hash
- [ ] Raw file remains human-readable for emergency recovery
- [ ] Updates append new ledger events (version DAG preserved)
- [ ] Renderer concatenates resolved entries in order
- [ ] Per-entry notes emitted as blockquote above entry content

## Plan

Library-first: most of the work is in `crates/kinora/src/kinograph.rs`; the CLI just wires store-time resolution through existing `Command::Store`.

**`crates/kinora/src/kinograph.rs` (new):**

- `Entry { id: String, name: Option<String>, pin: Option<String>, note: Option<String> }` — facet-derived, styx-parseable.
- `Kinograph { entries: Vec<Entry> }` — top-level with Vec<Entry>.
- `parse(bytes) -> Result<Kinograph, KinographError>`: parses styx, validates each entry (id 64-hex, pin 64-hex if present, name non-empty if present).
- `resolve_names(kinograph, resolver) -> Result<Kinograph, KinographError>`: for each entry whose `id` is empty OR doesn't look like a hash, treat the entry as name-only and resolve via `resolve_by_name`. Errors on ambiguous/missing names.
- `to_styx(kinograph) -> String`: round-trip serialization; raw file remains readable.
- `render(kinograph, resolver) -> Result<String, KinographError>`: fetches content for each entry (by pin if set, else current head), prefixes notes as blockquote lines, concatenates with blank-line separators. Returns the full rendered document as one String.
- `KinographError` variants: Parse, InvalidEntry { idx, reason }, Resolve (wraps ResolveError).

**CLI integration (`crates/kinora-cli/src/store.rs`):**

When `kind == "kinograph"`, after reading content but before calling `store_kino`:
1. `Kinograph::parse(&content)?`
2. `resolve_names(...)`
3. Re-serialize to styx → replaces the content bytes

Store-time rewrite means the saved content is authoritative (ids filled in) even if the user wrote names. The name/note hints are preserved for raw readability.

**Render: library only.** The mdbook CLI command lives in kinora-9nom. Here we only ship `render(kinograph, resolver)`.

Commit plan:
1. kinograph module: tests + parse/validate/to_styx
2. name resolution + render library primitive
3. CLI store-time rewrite
4. review fixes if any
