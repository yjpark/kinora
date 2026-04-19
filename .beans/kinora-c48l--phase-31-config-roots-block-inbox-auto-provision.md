---
# kinora-c48l
title: 'Phase 3.1: config roots {} block + inbox auto-provision'
status: in-progress
type: task
priority: normal
created_at: 2026-04-19T10:16:24Z
updated_at: 2026-04-19T10:47:18Z
parent: kinora-hxmw
---

Add RootPolicy enum and config.roots parsing; auto-provision inbox default.

First piece of phase 3 (kinora-hxmw). Introduces the config primitive for named roots with per-root policies so downstream children (compact, GC) have a declarative source of truth.

## Scope

### In scope

- [ ] `RootPolicy` enum: `Never`, `MaxAge(String)` (e.g. `"30d"`, `"12h"`), `KeepLastN(usize)`
- [ ] Parse policy strings: `"never"` → `Never`, `"30d"` / `"12h"` / `"7d"` → `MaxAge(_)`, `"keep-last-10"` → `KeepLastN(10)`. Reject unknown forms with a specific `ConfigError::InvalidPolicy` variant.
- [ ] Extend `Config` with `roots: BTreeMap<String, RootPolicy>` (BTreeMap so serialization order is canonical).
- [ ] Parse `roots { <name> { policy "<s>" } ... }` block in `config.styx` per D1 shape.
- [ ] Styx-level duplicate root names already error via facet's HashMap handling — verify with a test.
- [ ] Auto-provision default: if the parsed `roots {}` block doesn't declare `inbox`, `Config::from_styx` inserts `inbox → RootPolicy::MaxAge("30d")` before returning. Aggressive-by-default per §6.
- [ ] If the whole `roots {}` block is absent, treat as if only the default inbox is declared.
- [ ] `kinora init` writes the initial `config.styx` with an explicit `roots { inbox { policy "30d" } }` block so users see the shape.
- [ ] Tests: parse valid single/multi-root config, roundtrip, inbox auto-provision on missing block, inbox auto-provision when block present but no inbox, invalid policy string rejected, duplicate root name rejected.

### Out of scope (deferred)

- Using policies (GC lives in hxmw-6)
- Iterating roots at compact time (lives in hxmw-4)
- The `assign` event itself (lives in hxmw-3)

## Acceptance

- [ ] All sub-points under "In scope" implemented with tests
- [ ] Zero compiler warnings
- [ ] Bean todo items all checked off
- [ ] Summary of Changes section added at completion

## Plan

### Files to change

- `crates/kinora/src/config.rs` — new `RootPolicy` enum, policy string parser, `Config.roots: BTreeMap<String, RootPolicy>`, inbox auto-provision, new `ConfigError::InvalidPolicy` variant.
- `crates/kinora/src/init.rs` — write initial config with `roots { inbox { policy "30d" } }` block.

### Two-layer parse

Facet-derive a private `RawConfig { repo_url, roots: Option<BTreeMap<String, RawRootBlock>> }` for on-disk shape; hand-write public `Config { repo_url, roots: BTreeMap<String, RootPolicy> }` with `from_styx`/`to_styx` doing the raw→domain conversion. Keeps `RootPolicy` validation independent of facet_styx's derive mechanics and lets us produce specific error messages.

### Policy string grammar

- `"never"` → `RootPolicy::Never`
- `"keep-last-<N>"` where N parses as usize → `RootPolicy::KeepLastN(N)`
- `<digits><letters>` (e.g. `"30d"`, `"12h"`, `"1w"`) → `RootPolicy::MaxAge(<raw>)` — full duration parsing deferred to hxmw-6.
- Anything else → `ConfigError::InvalidPolicy { root, raw }`

### Inbox auto-provision

After parsing, `from_styx` checks whether `roots` contains `"inbox"` and inserts `RootPolicy::MaxAge("30d")` if not. Absent `roots {}` block treats as empty map; same outcome. Aggressive default per §6 nudges users to triage.

### Commit plan

1. **Tests commit**: stub `Config` with the new `roots` field but empty logic (always empty map, no inbox injection). Add every new test; confirm failures are assertion-based.
2. **Implementation commit**: RawConfig two-layer parse, policy grammar, inbox auto-provision. All tests pass; zero warnings.
3. **Review commit** (if needed): fixes from subagent review.
