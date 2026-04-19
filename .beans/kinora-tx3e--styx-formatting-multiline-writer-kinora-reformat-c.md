---
# kinora-tx3e
title: 'Styx formatting: multiline writer + kinora reformat command'
status: in-progress
type: feature
priority: normal
created_at: 2026-04-19T15:12:02Z
updated_at: 2026-04-19T19:10:27Z
blocked_by:
    - kinora-jezf
---

## Why

Styx files in the store are currently serialized with `facet_styx::to_string()` (auto formatting). For our data shape — kinograph kinos with an `entries` sequence — auto formatting collapses everything onto a single line that routinely exceeds a screen width. Example: `.kinora/store/55/552d1f8c72db188415380fcc29e0bcfbd5159c4bc0825bb48631c79df76821d9.styx` is one ~2500-char line.

Hard to read, hard to review in diffs, hard to debug. The fix is two parts:

1. **Writer switch**: all `to_styx()` callers pass `FormatOptions::default().multiline()`. New files come out newline-per-field.
2. **`kinora reformat` command**: for existing single-line files already in the store, walk them and stage new versions with reformatted content.

Part 1 alone is a one-liner change per call site but doesn't touch existing stored blobs. Part 2 migrates them.

## Part 1: writer switch

Call sites to update:

- `kinora/src/root.rs`: `RootKinograph::to_styx`
- `kinora/src/kinograph.rs`: kinograph serialization
- `kinora/src/config.rs`: `Config::to_styx`
- (verify: grep for `facet_styx::to_string` and `to_styx` in the workspace)

Use `FormatOptions::default().multiline()` at each site. Consider factoring into a single `kinora::styx::write(value)` helper so future tuning is one place.

Users with hand-edited `config.styx` are unaffected — we only write via `to_styx` from `init` and related codepaths.

## Part 2: `kinora reformat`

### CLI shape

```
kinora reformat
```

Respects the global `--repo-root` / `-C` flag. No other args initially.

### Semantics

Reformat is a *staged hash rotation* over the styx subset of the graph:

1. **Walk the reachable graph** from each root's current heads.
2. **For each reachable kino of kind `kinograph`** (regular kinograph kinos):
   - Read current content.
   - Parse → re-serialize with the new multiline formatter.
   - If the new bytes differ from the current bytes, stage a new-version event:
     - id = original kino's id (preserves identity)
     - version = hash of reformatted content
     - parents = [current version]
     - content = reformatted bytes
3. **For each reachable kino of kind `root`** (root kinograph kinos): specialized pathway — root kinographs are produced by commit (not staged), so reformat writes the new multiline content directly to the store and updates the root's head pointer to reference it. Skip if re-serialized bytes match current bytes.
4. **Skip markdown/text/other opaque kinds** — bytes are not styx, reformat doesn't apply. (User-driven content reformatting may be a separate feature later.)
5. **Config.styx is not a kino** — handled by the Part 1 writer switch, nothing for reformat to do.

### Why narrow scope works (no reference cascade)

Kinograph entries carry `pin: false` by default, which means parent kinographs reference children by identity and follow heads. After the user runs `kinora commit`, the newly staged reformatted versions become the heads of each lineage, and parent kinographs automatically resolve to them — no need for reformat to also restage the parents.

### Output

Summary to stdout:
- N styx kinos reformatted (staged)
- M styx kinos already in target format (skipped)
- Suggest next step: `kinora commit` to make the new versions heads

### "Dry-run" via staging

Staging without committing is itself the dry-run. Users can:
- Inspect `.kinora/staged/` (or equivalent post-rename) to see the new events
- Diff new vs old content manually
- Commit when satisfied, or clear staged to abort

No separate `--dry-run` flag is needed for end users. (If a developer-only preview mode becomes useful later, easy to add.)

### Single-kino mode

Deliberately not supported in the end-user CLI. Reformatting one kino in isolation produces a graph where only that kino has been updated — valid (parents head-track), but not generally useful. Could be useful as a developer debugging tool, but doesn't need to be in v1.

## Depends on

- `--repo-root` flag bean (kinora-jezf) — for consistent path plumbing across new commands
- Rename bean (kinora-2t6l, hot→staged, compact→commit) — *soft* dependency; the staging pathway exists by either name. If rename lands first, reformat uses the new vocabulary natively.

## Design decisions

- **Reformat rewrites root kinographs too**, via a specialized pathway. Waiting for a future natural commit would leave root kinographs stale indefinitely if no new events land on a given root, which defeats the user-visible point of reformat. So reformat directly writes new multiline root-kinograph kinos and updates root head pointers. Regular kinograph kinos still flow through the normal staged-new-version mechanism.
- **Idempotency**: skip any styx kino whose re-serialized bytes equal its current bytes. Running reformat twice on an already-multiline repo stages zero events and updates zero root pointers. Part of the summary output (`N skipped, already formatted`).

## Todos

- [ ] Add `kinora::styx::write` helper wrapping `facet_styx::to_string_with_options` with `FormatOptions::default().multiline()`
- [ ] Replace all `to_styx` / `facet_styx::to_string` call sites with the helper
- [ ] Verify new writes produce multiline output (unit test against a representative struct)
- [ ] Design `reformat` command module in `kinora-cli` + `kinora::reformat` in the library
- [ ] Implement graph walk + reformat detection
- [ ] Implement staged-event emission (reuse existing assign-event plumbing)
- [ ] Tests: reformat over a repo with mixed legacy + current-format kinos
- [ ] Tests: reformat is idempotent (running twice on an already-multiline repo stages nothing)
- [ ] Tests: after `kinora commit`, reformatted kinos are heads; `kinora render` picks them up
- [ ] Update docs / command help

## Acceptance

- All new styx writes produce multiline output
- `kinora reformat` on a repo with legacy single-line styx files stages new multiline versions; `kinora commit` lands them; `kinora render` shows unchanged user-visible output
- Idempotent on already-multiline repos (zero staged events)
- Zero compiler warnings, all tests pass

## Blocked: .multiline() is insufficient for nested inline-started structs

Marked back to draft during night shift after attempting Part 1 (writer switch). Empirical test proves the proposed approach does not achieve multiline output for our data shape.

### What was tried

Added `crates/kinora/src/styx.rs` with a helper:

```rust
pub fn to_string<'facet, T>(value: &T) -> Result<String, SerializeError<StyxSerializeError>>
where T: Facet<'facet> + ?Sized,
{
    let opts = SerializeOptions::default().multiline();
    to_string_with_options(value, &opts)
}
```

Switched all three call sites (`config.rs`, `root.rs`, `kinograph.rs`) to use it. Added a test asserting `RootKinograph { entries: [3 entries] }.to_styx()` produces `s.lines().count() > 1`.

The test FAILS. Output is a single line ~700 chars wide:

```
entries ({id 0101..., version 6565..., kind markdown, metadata {name name-1}, note "", pin false} {id 0202..., ...} {id 0303..., ...})
```

### Why it doesn't work

Inspected styx-format 3.0.2 writer.rs. At `begin_struct` (line 195), non-root structs are always created with `force_multiline: false` and `inline_start: true`. At `field_key` (line 326):

```rust
let struct_is_inline = inline_start && !force_multiline;
let should_inline = struct_is_inline || self.should_inline();
```

This short-circuits: `struct_is_inline = true` regardless of `ForceStyle::Multiline`. The writer's `should_inline()` check (which DOES respect `ForceStyle::Multiline` at line 131) is bypassed for any struct that started inline. Every non-root struct in our kinograph shape starts inline, so they all stay inline.

Same pattern for sequences at `begin_seq` (line 409): `inline_start: true` is hardcoded, and `before_value` (line 859) checks `inline_start || self.should_inline()` — `inline_start` wins.

Net effect: `FormatOptions::default().multiline()` only causes newlines between *direct* fields of the root struct. Nested struct/seq content stays inline.

### Why `format_source` post-pass also won't help

cst_format.rs:292 determines multiline layout purely from the parsed CST's separator tokens (`Separator::Newline | Separator::Mixed`). It does NOT consult `FormatOptions.force_style`. Since our current output uses comma separators everywhere, `format_source(serialized, FormatOptions::default().multiline())` would preserve the inline layout unchanged.

### Decision points for a future session

1. **Upstream fix:** file an issue / PR on `facet-styx` / `styx-format` so `ForceStyle::Multiline` actually forces multiline. Likely the cleanest path but requires coordination.
2. **Fork or vendor:** maintain a local patched copy of facet-styx with the fix.
3. **Post-process manually:** since our data shapes are known and simple, write a targeted pretty-printer (detect top-level seq, break entries onto separate lines, re-emit). Fragile.
4. **Wait for upstream:** park this work; readability isn't blocking core functionality.

Leaving for user to decide direction. Reverted uncommitted code changes (new `styx.rs`, call-site updates, failing test) so the tree stays clean.

## Revised plan: adopt styxl format for kinograph kinos

Supersedes the "Part 1 writer switch" approach above. That path depends on an upstream fix to facet-styx's `ForceStyle::Multiline` short-circuit, which is out of our hands.

### New approach

Kinograph kinos adopt a JSONL-style layout we call **styxl**: each line is an independently-parseable inline-form styx document for one entry. The file IS the sequence — no outer `entries ( ... )` wrapping.

Example (single entry per line):
```
{id 0dab...c017, version 0dab...c017, kind markdown, metadata {name identity-and-versions, tags design-principle, title "Identity and versions"}, note "", pin false}
{id 1c8b...fa46, version 1c8b...fa46, kind markdown, metadata {name provenance-mandatory, tags design-principle, title "Provenance is mandatory"}, note "", pin false}
```

### Why styxl beats the original writer-switch plan

- **No upstream dependency.** Per-entry inline serialization works correctly in facet-styx today.
- **Stream / grep / tail friendly.** `head -1 k.styxl` returns one valid entry; `grep 'id 0dab' *.styxl` finds hits directly.
- **Minimum-delta diffs.** No wrapper context lines; every git line-change is a semantic change.
- **Cheaper implementation.** Two small wrappers (`to_styxl` / `parse_styxl`) over `facet_styx::to_string` / `from_str` on `Entry`, instead of a new formatter path.

### Scope

- **Kinograph kinos** (kind `kinograph`, which produces `Kinograph` content) → styxl
- **Root kinographs** (`RootKinograph` content in the store) → styxl
- **Config.styx** → unchanged. Not a list-of-entries shape; single-line output is not a pain point.

### File extension

New extension: `.styxl`. Signals the new format to tooling, matches the jsonl precedent. Updates needed in whatever maps kind → extension. The store's `find_blob_path` already strips extensions when looking up blobs, so content-addressed lookups still work across the transition.

### Schema evolution (future)

YAGNI today — kinograph has only ever been `{ entries: Vec<Entry> }`. If we ever need a top-level header (schema version, description), reserve the first line of a styxl file for a header object distinguishable from entry objects (e.g. `{schema-version 2, ...}` — distinguishable by the absence of an `id` field). Documented as a future option, not built now.

### Revised Todos

- [x] Add `Kinograph::to_styxl` and `Kinograph::parse_styxl` in `kinograph.rs`; switch internal callers
- [x] Add `RootKinograph::to_styxl` and `RootKinograph::parse_styxl` in `root.rs`; switch internal callers
- [x] Switch kinograph-kind writes to produce `.styxl` blobs (update kind → extension mapping)
- [x] Tests: round-trip, empty entries, edge cases (escaped strings, long metadata)
- [x] Tests: per-line parsing independence (corrupt one line, others still load via a recovery path — or hard-fail cleanly; pick one and document)
- [x] Implement `kinora reformat`: walk reachable kinograph kinos, rewrite from `.styx` wrapped to `.styxl`, stage new-version events
- [x] Tests: reformat over a repo with legacy `.styx` kinos; idempotent after

### Revised acceptance

- Kinograph kinos are written as `.styxl` — one entry per line
- `kinora reformat` migrates existing `.styx` kinograph blobs to `.styxl` via staged new-version events
- After `kinora commit`, reformatted kinos are heads; `kinora render` shows unchanged user-visible output
- Idempotent on already-styxl repos (zero staged events)
- Zero compiler warnings, all tests pass
