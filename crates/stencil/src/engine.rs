//! The render/sync engine: the heart of stencil.
//!
//! Given a parsed [`StencilFile`] and a kinora [`Resolver`], the engine resolves
//! the file's `stencil:kinograph` binding to an api-kinograph, matches each
//! agent-placed slot to a kinograph entry **by name**, resolves that entry (its
//! `pin` if set, else the head), splits the spec kino into prose + code
//! ([`SpecItem`]), and writes the rendered read-only block beneath the slot.
//!
//! Guarantees:
//! - **Editable regions are preserved byte-for-byte** — only the read-only
//!   block owned by each slot is (re)written.
//! - **Idempotent** — re-running against unchanged sources is a no-op
//!   ([`SlotStatus::Unchanged`]); the read-only marker carries the source
//!   content hash so an unchanged source renders identically.
//! - **Drift is surfaced** — a read-only region edited by hand (same source
//!   hash, different content) is overwritten and reported
//!   ([`SlotStatus::DriftOverwritten`]).
//!
//! The engine is pure: it takes a parsed file and returns a new one plus a
//! [`SyncReport`]. File I/O and path scanning live in the CLI (`stencil sync`).

use std::collections::{BTreeMap, HashSet};
use std::str::FromStr;

use kinora::hash::Hash;
use kinora::kinograph::Kinograph;
use kinora::resolve::{Resolved, Resolver};

use crate::kinds;
use crate::region::{Block, StencilFile};
use crate::spec::SpecItem;
use crate::target::LanguageTarget;
use crate::StencilError;

// This module's public API is stencil-managed (dogfood, kinora-3guj): the
// SlotStatus / SlotOutcome / SyncReport / SyncOutcome types and the sync_file /
// kinograph_slot_names functions render into the read-only blocks below from
// the `stencil-engine-api` api-kinograph. Run `stencil sync` to refresh them;
// edit the kinos, not the blocks. Bodies and private helpers stay editable.

// stencil:kinograph stencil-engine-api

// stencil:slot engine-slot-status
// stencil:ro engine-slot-status 77ae5faa04c9da584039b4292919637201dacc967b62641375b3c4a924bdfa6a
/// What happened to a single slot's read-only block during a sync.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotStatus {
    /// The slot had no read-only block; one was created.
    Created,
    /// The block existed but its source kino changed; it was refreshed.
    Updated,
    /// The block already matched the resolved source; nothing changed.
    Unchanged,
    /// The block's read-only region had been hand-edited (same source hash,
    /// different content); stencil overwrote it.
    DriftOverwritten,
    /// The slot names no entry in the bound api-kinograph; left untouched.
    Unmatched,
}
// stencil:end

// stencil:slot engine-slot-outcome
// stencil:ro engine-slot-outcome bdfd17afebe1ecd909200106d607de381dcedfd247fea7f6f3f2b35f51d0cf96
/// The outcome for one slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotOutcome {
    pub name: String,
    pub status: SlotStatus,
}
// stencil:end

// stencil:slot engine-sync-report
// stencil:ro engine-sync-report 4457e3b0c104dc01eab3a90bf3000ddde60afcb4b6cf0aadac86486120e980ff
/// A summary of a single file's sync.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SyncReport {
    /// Per-slot outcomes, in document order.
    pub slots: Vec<SlotOutcome>,
    /// Kinograph entries that no slot in the file claims (sorted by name).
    pub unslotted_entries: Vec<String>,
    /// Read-only blocks with no owning slot above them (sorted by name).
    pub orphans: Vec<String>,
}
// stencil:end

impl SyncReport {
    // stencil:slot engine-sync-report-changed
    // stencil:ro engine-sync-report-changed a1d68a81286f5e2ac99423ebc68160242cd9bac19428004b5362da116577aa05
    /// Whether the sync altered the file (any block created, updated, or
    /// drift-overwritten). The CLI writes the file back only when true.
    pub fn changed(&self) -> bool
    // stencil:end
    {
        self.slots.iter().any(|s| {
            matches!(
                s.status,
                SlotStatus::Created | SlotStatus::Updated | SlotStatus::DriftOverwritten
            )
        })
    }

    // stencil:slot engine-sync-report-drifted
    // stencil:ro engine-sync-report-drifted f2058ed09257e69ed0410f872cd73d08f9fbfe27d7742f40310ec2f5c6a6f395
    /// Names of slots whose read-only regions were hand-edited and overwritten.
    pub fn drifted(&self) -> Vec<&str>
    // stencil:end
    {
        self.slots
            .iter()
            .filter(|s| s.status == SlotStatus::DriftOverwritten)
            .map(|s| s.name.as_str())
            .collect()
    }

    // stencil:slot engine-sync-report-unmatched
    // stencil:ro engine-sync-report-unmatched 8beaa3b7c212dcbe829344e1b51b1cafe7973c51863eb6ca946f57a9af6b2069
    /// Names of slots that matched no kinograph entry.
    pub fn unmatched(&self) -> Vec<&str>
    // stencil:end
    {
        self.slots
            .iter()
            .filter(|s| s.status == SlotStatus::Unmatched)
            .map(|s| s.name.as_str())
            .collect()
    }
}

// stencil:slot engine-sync-outcome
// stencil:ro engine-sync-outcome 9ea0b0502ec3d2695cfcdaa294612f67e40ab0eec3b35681f3ed1ef0e429e4f7
/// The result of [`sync_file`]: the updated file plus its report.
#[derive(Debug, Clone)]
pub struct SyncOutcome {
    pub file: StencilFile,
    pub report: SyncReport,
}
// stencil:end

// stencil:slot engine-sync-file
// stencil:ro engine-sync-file 31565b160952e02a486c7c81de9833cfea4792bb537dda8626f604ab3a4ff6cf
/// Sync one file against `resolver`, rendering read-only blocks for the
/// language `target`. Returns the updated file and a report; the input is not
/// mutated.
pub fn sync_file(
    file: &StencilFile,
    resolver: &Resolver,
    target: &dyn LanguageTarget,
) -> Result<SyncOutcome, StencilError>
// stencil:end
{
    let slot_count = file
        .blocks
        .iter()
        .filter(|b| matches!(b, Block::Slot { .. }))
        .count();

    // No slots → nothing to render. Report any orphan read-only blocks and
    // return the file untouched (this also covers files with no markers).
    if slot_count == 0 {
        let orphans = file
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::ReadOnly { name, .. } => Some(name.clone()),
                _ => None,
            })
            .collect();
        return Ok(SyncOutcome {
            file: file.clone(),
            report: SyncReport { orphans, ..Default::default() },
        });
    }

    let reference = file.binding().ok_or(StencilError::NoBinding)?.to_owned();
    let index = build_index(&reference, resolver)?;

    let mut new_blocks: Vec<Block> = Vec::with_capacity(file.blocks.len());
    let mut report = SyncReport::default();
    let mut matched: HashSet<String> = HashSet::new();

    let mut i = 0;
    while i < file.blocks.len() {
        match &file.blocks[i] {
            Block::Slot { name, indent } => {
                new_blocks.push(file.blocks[i].clone());

                // A slot owns the immediately-following read-only block iff its
                // name matches.
                let existing = match file.blocks.get(i + 1) {
                    Some(b @ Block::ReadOnly { name: n2, .. }) if n2 == name => {
                        i += 1;
                        Some(b)
                    }
                    _ => None,
                };

                match index.get(name) {
                    None => {
                        report.slots.push(SlotOutcome {
                            name: name.clone(),
                            status: SlotStatus::Unmatched,
                        });
                        // Keep any existing block — don't destroy content on a
                        // transient lookup miss.
                        if let Some(ex) = existing {
                            new_blocks.push(ex.clone());
                        }
                    }
                    Some(resolved) => {
                        matched.insert(name.clone());
                        // Kind validation + spec parsing are deferred to here so
                        // a broken *unrelated* entry never fails this file.
                        let block = render_entry(resolved, name, indent, target)?;
                        let status = classify(existing, &block);
                        new_blocks.push(block);
                        report.slots.push(SlotOutcome { name: name.clone(), status });
                    }
                }
            }
            Block::ReadOnly { name, .. } => {
                // A read-only block not consumed by a preceding matching slot is
                // an orphan (its slot was moved or removed). Keep it, report it.
                report.orphans.push(name.clone());
                new_blocks.push(file.blocks[i].clone());
            }
            _ => new_blocks.push(file.blocks[i].clone()),
        }
        i += 1;
    }

    report.unslotted_entries = index
        .keys()
        .filter(|name| !matched.contains(*name))
        .cloned()
        .collect();
    // BTreeMap keys already iterate sorted, but be explicit.
    report.unslotted_entries.sort();
    report.orphans.sort();

    Ok(SyncOutcome { file: StencilFile { blocks: new_blocks }, report })
}

/// Resolve the api-kinograph named by `reference` into ordered `(name,
/// Resolved)` pairs, in kinograph (document) order.
///
/// Resolution is *tolerant per entry*: an entry that fails to resolve (e.g. an
/// unrelated fork or bad pin) is skipped rather than failing the whole call —
/// kind validation and spec parsing are deferred to [`render_entry`], which
/// only runs for the entries a slot actually claims. Kinograph-level problems
/// (missing, wrong kind, parse, ambiguous names) are still loud.
///
/// Two entries resolving to the same name is a kinograph defect that makes
/// slot-matching ambiguous, so it fails with [`StencilError::DuplicateEntryName`]
/// (mirroring kinora's fail-loud-on-ambiguity convention).
fn resolve_entries(
    reference: &str,
    resolver: &Resolver,
) -> Result<Vec<(String, Resolved)>, StencilError> {
    let kg_resolved = resolve_reference(resolver, reference)?;
    if kg_resolved.head.kind != kinds::API_KINOGRAPH {
        return Err(StencilError::NotApiKinograph {
            reference: reference.to_owned(),
            kind: kg_resolved.head.kind.clone(),
        });
    }
    let kg = Kinograph::parse(&kg_resolved.content)?.resolve_names(resolver)?;

    let mut seen: HashSet<String> = HashSet::new();
    let mut entries: Vec<(String, Resolved)> = Vec::new();
    for entry in &kg.entries {
        let resolved = match entry.pin_opt() {
            Some(pin) => resolver.resolve_at_version(&entry.id, pin),
            None => resolver.resolve_by_id(&entry.id),
        };
        let Ok(resolved) = resolved else { continue };
        let name = entry_name(&resolved, entry);
        if !seen.insert(name.clone()) {
            return Err(StencilError::DuplicateEntryName { name });
        }
        entries.push((name, resolved));
    }
    Ok(entries)
}

/// Resolve the api-kinograph named by `reference` and index its entries by
/// name. See [`resolve_entries`] for resolution semantics.
fn build_index(
    reference: &str,
    resolver: &Resolver,
) -> Result<BTreeMap<String, Resolved>, StencilError> {
    Ok(resolve_entries(reference, resolver)?.into_iter().collect())
}

// stencil:slot engine-kinograph-slot-names
// stencil:ro engine-kinograph-slot-names f3814390dc44fe4bbb780693672237fe4e79d8d383e6c35e6e31931df619cfd9
/// The entry names of an api-kinograph, in kinograph (document) order — the
/// slots `stencil scaffold` should emit, one per entry.
///
/// Mirrors [`sync_file`]'s resolution exactly (see [`resolve_entries`]):
/// unresolvable entries are skipped, so every returned name is one a freshly
/// scaffolded slot will match; duplicate names are a hard error.
///
/// A slot marker is a single whitespace-free token, so an entry whose resolved
/// name is empty or contains whitespace can never be slotted (`sync` would
/// leave it unmatched, and a scaffolded marker would be unparseable). Rather
/// than emit a broken skeleton, this fails loud with
/// [`StencilError::UnslottableEntryName`].
pub fn kinograph_slot_names(
    reference: &str,
    resolver: &Resolver,
) -> Result<Vec<String>, StencilError>
// stencil:end
{
    let mut names = Vec::new();
    for (name, _) in resolve_entries(reference, resolver)? {
        if name.is_empty() || name.chars().any(char::is_whitespace) {
            return Err(StencilError::UnslottableEntryName { name });
        }
        names.push(name);
    }
    Ok(names)
}

/// Render a slotted entry's read-only block: validate it is an api-spec, split
/// its markdown, and compose the indented doc-comment + signature content.
fn render_entry(
    resolved: &Resolved,
    name: &str,
    indent: &str,
    target: &dyn LanguageTarget,
) -> Result<Block, StencilError> {
    if resolved.head.kind != kinds::API_SPEC {
        return Err(StencilError::NotApiSpec {
            name: name.to_owned(),
            kind: resolved.head.kind.clone(),
        });
    }
    let item = SpecItem::from_bytes(&resolved.content)?;
    let content = apply_indent(&render_content(&item, target), indent);
    Ok(Block::read_only(name.to_owned(), resolved.head.hash.clone(), content, indent.to_owned()))
}

/// Resolve a kino reference that is either a 64-hex id or a metadata name.
fn resolve_reference(resolver: &Resolver, reference: &str) -> Result<Resolved, StencilError> {
    let resolved = if Hash::from_str(reference).is_ok() {
        resolver.resolve_by_id(reference)?
    } else {
        resolver.resolve_by_name(reference)?
    };
    Ok(resolved)
}

/// The canonical name of a resolved entry: its head `metadata.name`, falling
/// back to the kinograph entry's name hint, then its id.
///
/// For a pinned entry this reads the *pinned* version's metadata, so a kino
/// renamed after it was pinned is indexed under its old name; the entry-hint
/// fallback softens this.
fn entry_name(resolved: &Resolved, entry: &kinora::kinograph::Entry) -> String {
    resolved
        .head
        .metadata
        .get("name")
        .cloned()
        .or_else(|| entry.name_opt().map(str::to_owned))
        .unwrap_or_else(|| resolved.id.clone())
}

/// Render a spec item's read-only content: doc-comments from the prose contract
/// directly above the signature code. Either part may be absent.
fn render_content(item: &SpecItem, target: &dyn LanguageTarget) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    if !item.doc_prose.is_empty() {
        lines.extend(target.doc_comment(&item.doc_prose).split('\n').map(str::to_owned));
    }
    if item.has_code() {
        lines.extend(item.code().split('\n').map(str::to_owned));
    }
    lines
}

/// Indent every non-empty line by `indent` (blank lines stay blank).
fn apply_indent(lines: &[String], indent: &str) -> Vec<String> {
    if indent.is_empty() {
        return lines.to_vec();
    }
    lines
        .iter()
        .map(|l| if l.is_empty() { String::new() } else { format!("{indent}{l}") })
        .collect()
}

/// Classify a freshly-rendered read-only block against any existing one. Both
/// carry the source content hash they were rendered from.
fn classify(existing: Option<&Block>, fresh: &Block) -> SlotStatus {
    let Some(existing) = existing else {
        return SlotStatus::Created;
    };
    if existing == fresh {
        return SlotStatus::Unchanged;
    }
    let existing_hash = block_hash(existing);
    let fresh_hash = block_hash(fresh);
    // Same source version but different bytes ⇒ the region was hand-edited.
    if existing_hash == fresh_hash {
        SlotStatus::DriftOverwritten
    } else {
        SlotStatus::Updated
    }
}

/// The source hash a read-only block was rendered from (empty for other kinds).
fn block_hash(block: &Block) -> &str {
    match block {
        Block::ReadOnly { hash, .. } => hash.as_str(),
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::target::RustTarget;
    use kinora::init::init;
    use kinora::kino::{store_kino, StoreKinoParams};
    use kinora::paths::kinora_root;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn setup() -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        init(tmp.path(), "https://example.com/x.git").unwrap();
        let root = kinora_root(tmp.path());
        (tmp, root)
    }

    fn params(kind: &str, content: &[u8], name: &str) -> StoreKinoParams {
        StoreKinoParams {
            kind: kind.into(),
            content: content.to_vec(),
            author: "t".into(),
            provenance: "t".into(),
            ts: "2026-06-10T10:00:00Z".into(),
            metadata: BTreeMap::from([("name".into(), name.into())]),
            id: None,
            parents: vec![],
        }
    }

    fn store_spec(root: &std::path::Path, name: &str, md: &str) -> kinora::event::Event {
        store_kino(root, params(kinds::API_SPEC, md.as_bytes(), name)).unwrap().event
    }

    /// Store an api-kinograph kino composing the given entries.
    fn store_kinograph(
        root: &std::path::Path,
        name: &str,
        entries: Vec<kinora::kinograph::Entry>,
    ) -> kinora::event::Event {
        let kg = kinora::kinograph::Kinograph { entries };
        let content = kg.to_styxl().unwrap();
        store_kino(root, params(kinds::API_KINOGRAPH, content.as_bytes(), name))
            .unwrap()
            .event
    }

    fn parse(src: &str) -> StencilFile {
        StencilFile::parse(src, &RustTarget).unwrap()
    }

    fn sync(file: &StencilFile, root: &std::path::Path) -> SyncOutcome {
        let resolver = Resolver::load(root).unwrap();
        sync_file(file, &resolver, &RustTarget).unwrap()
    }

    const SPEC_MD: &str =
        "Creates a user. Errors if the name is empty.\n\n```rust\npub fn new(name: &str) -> Result<User, UserError>;\n```\n";

    #[test]
    fn fills_an_empty_slot() {
        let (_t, root) = setup();
        let spec = store_spec(&root, "user-new", SPEC_MD);
        store_kinograph(
            &root,
            "user-api",
            vec![kinora::kinograph::Entry::with_id(spec.id.clone())],
        );

        let file = parse("// stencil:kinograph user-api\n// stencil:slot user-new\n");
        let out = sync(&file, &root);

        assert_eq!(out.report.slots, vec![SlotOutcome {
            name: "user-new".into(),
            status: SlotStatus::Created,
        }]);
        assert!(out.report.changed());
        let src = out.file.to_source(&RustTarget);
        assert!(src.contains("/// Creates a user. Errors if the name is empty."));
        assert!(src.contains("pub fn new(name: &str) -> Result<User, UserError>;"));
        assert!(src.contains(&format!("// stencil:ro user-new {}", spec.hash)));
    }

    #[test]
    fn second_sync_is_a_noop() {
        let (_t, root) = setup();
        let spec = store_spec(&root, "user-new", SPEC_MD);
        store_kinograph(&root, "user-api", vec![kinora::kinograph::Entry::with_id(spec.id)]);

        let file = parse("// stencil:kinograph user-api\n// stencil:slot user-new\n");
        let first = sync(&file, &root);
        let second = sync(&first.file, &root);

        assert_eq!(second.report.slots[0].status, SlotStatus::Unchanged);
        assert!(!second.report.changed());
        assert_eq!(first.file, second.file);
    }

    #[test]
    fn editable_regions_are_preserved_byte_for_byte() {
        let (_t, root) = setup();
        let spec = store_spec(&root, "user-new", SPEC_MD);
        store_kinograph(&root, "user-api", vec![kinora::kinograph::Entry::with_id(spec.id)]);

        let src = concat!(
            "// stencil:kinograph user-api\n",
            "use crate::error::UserError;\n",
            "\n",
            "// stencil:slot user-new\n",
            "{\n",
            "    // hand-written body — must survive\n",
            "    todo!()\n",
            "}\n",
        );
        let out = sync(&parse(src), &root);
        let result = out.file.to_source(&RustTarget);
        assert!(result.contains("use crate::error::UserError;"));
        assert!(result.contains("    // hand-written body — must survive"));
        assert!(result.contains("    todo!()"));
        // The read-only block was inserted between the slot and the editable body.
        let ro_pos = result.find("stencil:ro").unwrap();
        let body_pos = result.find("hand-written body").unwrap();
        assert!(ro_pos < body_pos);
    }

    #[test]
    fn updates_when_source_kino_changes() {
        let (_t, root) = setup();
        let v1 = store_spec(&root, "user-new", SPEC_MD);
        store_kinograph(&root, "user-api", vec![kinora::kinograph::Entry::with_id(v1.id.clone())]);

        let file = parse("// stencil:kinograph user-api\n// stencil:slot user-new\n");
        let first = sync(&file, &root);

        // Store v2 of the spec kino (new signature).
        let mut p = params(
            kinds::API_SPEC,
            b"Creates a user.\n\n```rust\npub fn new(name: &str, age: u8) -> Result<User, UserError>;\n```\n",
            "user-new",
        );
        p.id = Some(v1.id.clone());
        p.parents = vec![v1.hash.clone()];
        p.ts = "2026-06-10T11:00:00Z".into();
        let v2 = store_kino(&root, p).unwrap().event;

        let second = sync(&first.file, &root);
        assert_eq!(second.report.slots[0].status, SlotStatus::Updated);
        let src = second.file.to_source(&RustTarget);
        assert!(src.contains("age: u8"));
        assert!(src.contains(&format!("// stencil:ro user-new {}", v2.hash)));
        assert!(!src.contains("(name: &str) -> Result<User, UserError>;"));
    }

    #[test]
    fn hand_edited_read_only_region_is_drift_overwritten() {
        let (_t, root) = setup();
        let spec = store_spec(&root, "user-new", SPEC_MD);
        store_kinograph(&root, "user-api", vec![kinora::kinograph::Entry::with_id(spec.id)]);

        let file = parse("// stencil:kinograph user-api\n// stencil:slot user-new\n");
        let synced = sync(&file, &root).file.to_source(&RustTarget);

        // Hand-edit the signature inside the read-only region (hash unchanged).
        let tampered = synced.replace(
            "pub fn new(name: &str) -> Result<User, UserError>;",
            "pub fn new() -> User; // sneaky edit",
        );
        assert_ne!(tampered, synced);

        let out = sync(&parse(&tampered), &root);
        assert_eq!(out.report.slots[0].status, SlotStatus::DriftOverwritten);
        assert_eq!(out.report.drifted(), vec!["user-new"]);
        let restored = out.file.to_source(&RustTarget);
        assert!(restored.contains("pub fn new(name: &str) -> Result<User, UserError>;"));
        assert!(!restored.contains("sneaky edit"));
    }

    #[test]
    fn slot_with_no_matching_entry_is_unmatched() {
        let (_t, root) = setup();
        let spec = store_spec(&root, "user-new", SPEC_MD);
        store_kinograph(&root, "user-api", vec![kinora::kinograph::Entry::with_id(spec.id)]);

        let file = parse("// stencil:kinograph user-api\n// stencil:slot does-not-exist\n");
        let out = sync(&file, &root);
        assert_eq!(out.report.slots[0].status, SlotStatus::Unmatched);
        assert_eq!(out.report.unmatched(), vec!["does-not-exist"]);
        assert!(!out.report.changed());
    }

    #[test]
    fn entries_without_slots_are_collected() {
        let (_t, root) = setup();
        let a = store_spec(&root, "user-new", SPEC_MD);
        let b = store_spec(
            &root,
            "user-find",
            "Finds a user by id.\n\n```rust\npub fn find(id: u64) -> Option<User>;\n```\n",
        );
        store_kinograph(
            &root,
            "user-api",
            vec![
                kinora::kinograph::Entry::with_id(a.id),
                kinora::kinograph::Entry::with_id(b.id),
            ],
        );

        let file = parse("// stencil:kinograph user-api\n// stencil:slot user-new\n");
        let out = sync(&file, &root);
        assert_eq!(out.report.unslotted_entries, vec!["user-find"]);
    }

    #[test]
    fn pinned_entry_renders_the_pinned_version() {
        let (_t, root) = setup();
        let v1 = store_spec(&root, "user-new", SPEC_MD);
        // v2 with a different signature.
        let mut p = params(
            kinds::API_SPEC,
            b"Doc.\n\n```rust\npub fn new(name: &str, age: u8) -> User;\n```\n",
            "user-new",
        );
        p.id = Some(v1.id.clone());
        p.parents = vec![v1.hash.clone()];
        p.ts = "2026-06-10T11:00:00Z".into();
        store_kino(&root, p).unwrap();

        // Kinograph pins the entry to v1.
        let pinned = kinora::kinograph::Entry {
            id: v1.id.clone(),
            name: String::new(),
            pin: v1.hash.clone(),
            note: String::new(),
        };
        store_kinograph(&root, "user-api", vec![pinned]);

        let file = parse("// stencil:kinograph user-api\n// stencil:slot user-new\n");
        let out = sync(&file, &root);
        let src = out.file.to_source(&RustTarget);
        assert!(src.contains("pub fn new(name: &str) -> Result<User, UserError>;"));
        assert!(!src.contains("age: u8"), "pin should hold v1, got: {src}");
    }

    #[test]
    fn slots_inherit_indentation() {
        let (_t, root) = setup();
        let spec = store_spec(&root, "user-new", SPEC_MD);
        store_kinograph(&root, "user-api", vec![kinora::kinograph::Entry::with_id(spec.id)]);

        let src = concat!(
            "// stencil:kinograph user-api\n",
            "mod user {\n",
            "    // stencil:slot user-new\n",
            "}\n",
        );
        let out = sync(&parse(src), &root);
        let result = out.file.to_source(&RustTarget);
        assert!(result.contains("    // stencil:ro user-new"));
        assert!(result.contains("    /// Creates a user. Errors if the name is empty."));
        assert!(result.contains("    pub fn new(name: &str) -> Result<User, UserError>;"));
        assert!(result.contains("    // stencil:end"));
    }

    #[test]
    fn missing_binding_with_slots_errors() {
        let (_t, root) = setup();
        let file = parse("// stencil:slot user-new\n");
        let resolver = Resolver::load(&root).unwrap();
        let err = sync_file(&file, &resolver, &RustTarget).unwrap_err();
        assert!(matches!(err, StencilError::NoBinding));
    }

    #[test]
    fn binding_to_non_kinograph_kind_errors() {
        let (_t, root) = setup();
        store_kino(&root, params("markdown", b"just docs", "user-api")).unwrap();
        let spec = store_spec(&root, "user-new", SPEC_MD);
        let _ = spec;

        let file = parse("// stencil:kinograph user-api\n// stencil:slot user-new\n");
        let resolver = Resolver::load(&root).unwrap();
        let err = sync_file(&file, &resolver, &RustTarget).unwrap_err();
        assert!(matches!(err, StencilError::NotApiKinograph { .. }), "got: {err:?}");
    }

    #[test]
    fn kinograph_entry_pointing_at_non_spec_errors() {
        let (_t, root) = setup();
        // A markdown kino masquerading as an api-spec entry.
        let md = store_kino(&root, params("markdown", b"not a spec", "user-new")).unwrap().event;
        store_kinograph(&root, "user-api", vec![kinora::kinograph::Entry::with_id(md.id)]);

        let file = parse("// stencil:kinograph user-api\n// stencil:slot user-new\n");
        let resolver = Resolver::load(&root).unwrap();
        let err = sync_file(&file, &resolver, &RustTarget).unwrap_err();
        assert!(matches!(err, StencilError::NotApiSpec { .. }), "got: {err:?}");
    }

    #[test]
    fn file_without_markers_is_a_noop() {
        let (_t, root) = setup();
        let file = parse("fn main() {\n    println!(\"hi\");\n}\n");
        let out = sync(&file, &root);
        assert!(out.report.slots.is_empty());
        assert!(!out.report.changed());
        assert_eq!(out.file, file);
    }

    #[test]
    fn orphan_read_only_block_is_reported_and_kept() {
        let (_t, root) = setup();
        let spec = store_spec(&root, "user-new", SPEC_MD);
        store_kinograph(&root, "user-api", vec![kinora::kinograph::Entry::with_id(spec.id)]);

        // A read-only block with no slot above it.
        let src = concat!(
            "// stencil:kinograph user-api\n",
            "// stencil:ro orphaned abc\n",
            "pub fn gone();\n",
            "// stencil:end\n",
            "// stencil:slot user-new\n",
        );
        let out = sync(&parse(src), &root);
        assert_eq!(out.report.orphans, vec!["orphaned"]);
        // The orphan content is kept, not destroyed.
        assert!(out.file.to_source(&RustTarget).contains("pub fn gone();"));
    }

    #[test]
    fn unrelated_non_spec_entry_does_not_block_slotted_sync() {
        let (_t, root) = setup();
        let good = store_spec(&root, "user-new", SPEC_MD);
        let legacy = store_kino(&root, params("markdown", b"not a spec", "legacy"))
            .unwrap()
            .event;
        store_kinograph(
            &root,
            "user-api",
            vec![
                kinora::kinograph::Entry::with_id(good.id),
                kinora::kinograph::Entry::with_id(legacy.id),
            ],
        );

        // Only the healthy entry is slotted; the non-spec entry must not abort.
        let file = parse("// stencil:kinograph user-api\n// stencil:slot user-new\n");
        let out = sync(&file, &root);
        assert_eq!(out.report.slots[0].status, SlotStatus::Created);
        assert!(out.file.to_source(&RustTarget).contains("pub fn new(name: &str)"));
        assert!(out.report.unslotted_entries.contains(&"legacy".to_string()));
    }

    #[test]
    fn unrelated_forked_entry_is_tolerated() {
        let (_t, root) = setup();
        let good = store_spec(&root, "user-new", SPEC_MD);

        // A forked spec: v1 with two sibling children off the same parent.
        let v1 = store_spec(&root, "forked", "Doc.\n\n```rust\npub fn f();\n```\n");
        for (content, ts) in [
            (b"Doc.\n\n```rust\npub fn left();\n```\n".as_slice(), "2026-06-10T11:00:00Z"),
            (b"Doc.\n\n```rust\npub fn right();\n```\n", "2026-06-10T11:00:01Z"),
        ] {
            let mut p = params(kinds::API_SPEC, content, "forked");
            p.id = Some(v1.id.clone());
            p.parents = vec![v1.hash.clone()];
            p.ts = ts.into();
            store_kino(&root, p).unwrap();
        }
        store_kinograph(
            &root,
            "user-api",
            vec![
                kinora::kinograph::Entry::with_id(good.id),
                kinora::kinograph::Entry::with_id(v1.id),
            ],
        );

        let file = parse("// stencil:kinograph user-api\n// stencil:slot user-new\n");
        let out = sync(&file, &root); // the unrelated fork must not block
        assert_eq!(out.report.slots[0].status, SlotStatus::Created);
        // The forked entry is unresolvable, so it's skipped entirely.
        assert!(!out.report.unslotted_entries.iter().any(|n| n == "forked"));
    }

    #[test]
    fn duplicate_entry_names_error() {
        let (_t, root) = setup();
        let a = store_spec(&root, "dup", "Doc A.\n\n```rust\npub fn a();\n```\n");
        let b = store_spec(&root, "dup", "Doc B.\n\n```rust\npub fn b();\n```\n");
        store_kinograph(
            &root,
            "user-api",
            vec![
                kinora::kinograph::Entry::with_id(a.id),
                kinora::kinograph::Entry::with_id(b.id),
            ],
        );

        let file = parse("// stencil:kinograph user-api\n// stencil:slot dup\n");
        let resolver = Resolver::load(&root).unwrap();
        let err = sync_file(&file, &resolver, &RustTarget).unwrap_err();
        assert!(matches!(err, StencilError::DuplicateEntryName { .. }), "got: {err:?}");
    }

    #[test]
    fn slot_names_preserve_kinograph_order() {
        let (_t, root) = setup();
        // Store in one order; reference them in a different kinograph order.
        let find = store_spec(
            &root,
            "user-find",
            "Finds a user.\n\n```rust\npub fn find(id: u64) -> Option<User>;\n```\n",
        );
        let new = store_spec(&root, "user-new", SPEC_MD);
        store_kinograph(
            &root,
            "user-api",
            vec![
                kinora::kinograph::Entry::with_id(new.id),
                kinora::kinograph::Entry::with_id(find.id),
            ],
        );

        let resolver = Resolver::load(&root).unwrap();
        let names = kinograph_slot_names("user-api", &resolver).unwrap();
        // Document order, not alphabetical (which would put user-find first).
        assert_eq!(names, vec!["user-new".to_string(), "user-find".into()]);
    }

    #[test]
    fn slot_names_skip_unresolvable_entries() {
        let (_t, root) = setup();
        let good = store_spec(&root, "user-new", SPEC_MD);
        // A forked entry resolves ambiguously and is skipped.
        let v1 = store_spec(&root, "forked", "Doc.\n\n```rust\npub fn f();\n```\n");
        for (content, ts) in [
            (b"Doc.\n\n```rust\npub fn left();\n```\n".as_slice(), "2026-06-10T11:00:00Z"),
            (b"Doc.\n\n```rust\npub fn right();\n```\n", "2026-06-10T11:00:01Z"),
        ] {
            let mut p = params(kinds::API_SPEC, content, "forked");
            p.id = Some(v1.id.clone());
            p.parents = vec![v1.hash.clone()];
            p.ts = ts.into();
            store_kino(&root, p).unwrap();
        }
        store_kinograph(
            &root,
            "user-api",
            vec![
                kinora::kinograph::Entry::with_id(good.id),
                kinora::kinograph::Entry::with_id(v1.id),
            ],
        );

        let resolver = Resolver::load(&root).unwrap();
        let names = kinograph_slot_names("user-api", &resolver).unwrap();
        assert_eq!(names, vec!["user-new".to_string()]);
    }

    #[test]
    fn slot_names_error_on_duplicate() {
        let (_t, root) = setup();
        let a = store_spec(&root, "dup", "Doc A.\n\n```rust\npub fn a();\n```\n");
        let b = store_spec(&root, "dup", "Doc B.\n\n```rust\npub fn b();\n```\n");
        store_kinograph(
            &root,
            "user-api",
            vec![
                kinora::kinograph::Entry::with_id(a.id),
                kinora::kinograph::Entry::with_id(b.id),
            ],
        );

        let resolver = Resolver::load(&root).unwrap();
        let err = kinograph_slot_names("user-api", &resolver).unwrap_err();
        assert!(matches!(err, StencilError::DuplicateEntryName { .. }), "got: {err:?}");
    }

    #[test]
    fn slot_names_reject_non_kinograph() {
        let (_t, root) = setup();
        store_kino(&root, params("markdown", b"just docs", "user-api")).unwrap();
        let resolver = Resolver::load(&root).unwrap();
        let err = kinograph_slot_names("user-api", &resolver).unwrap_err();
        assert!(matches!(err, StencilError::NotApiKinograph { .. }), "got: {err:?}");
    }

    #[test]
    fn slot_names_error_on_whitespace_name() {
        let (_t, root) = setup();
        // A spec whose metadata name has a space cannot be a single-token slot.
        let spec = store_spec(&root, "user new", SPEC_MD);
        store_kinograph(
            &root,
            "user-api",
            vec![kinora::kinograph::Entry::with_id(spec.id)],
        );

        let resolver = Resolver::load(&root).unwrap();
        let err = kinograph_slot_names("user-api", &resolver).unwrap_err();
        assert!(matches!(err, StencilError::UnslottableEntryName { .. }), "got: {err:?}");
    }
}
