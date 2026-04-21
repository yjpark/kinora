//! Root kinograph: a `kind: root` content blob. Its styxl document is
//! a header line (commit metadata) followed by one entry per line.
//!
//! ```text
//! {id <lineage-id>, parents (<prior>, ...), ts <rfc3339>, author "...", provenance "..."}
//! {id <entry-id>, version <hash>, kind markdown, metadata {...}, note "", pin false, head_ts <ts>}
//! {id <entry-id>, ...}
//! ```
//!
//! The header's `id` is the root's **lineage id** — stable across every
//! version of the same named root. Genesis derives it from
//! `hash(entries-only-bytes)`; subsequent versions carry it forward
//! unchanged. `parents` lists the blob hashes of the prior root
//! version(s); empty on genesis.
//!
//! Root kinographs differ structurally from composition kinographs
//! (`crate::kinograph::Kinograph`): composition entries are pure
//! `{id, name?, pin?, note?}` pointers, root entries are the metadata
//! home. The parser/serializer lives in this separate module so the
//! field sets don't leak between the two shapes.
//!
//! Canonical form: entries sorted by `id` (ascii-hex), metadata keys
//! sorted (`BTreeMap` handles this). `to_styxl` always emits canonical.
//!
//! Hard cutover: the pre-et1t legacy forms (`entries (…)` styx-wrapped
//! and header-less styxl) no longer parse. Repos predating this change
//! must be rebuilt from source — history across the cutover is not
//! preserved. Kinora is pre-1.0.

use std::collections::BTreeMap;
use std::collections::HashSet;
use std::str::FromStr;

use facet::Facet;

use crate::hash::Hash;
use crate::namespace::{self, NamespaceError};

#[derive(Facet, Debug, Clone, PartialEq, Eq)]
pub struct RootEntry {
    pub id: String,
    pub version: String,
    pub kind: String,
    pub metadata: BTreeMap<String, String>,
    #[facet(default)]
    pub note: String,
    #[facet(default)]
    pub pin: bool,
    // RFC3339 ts of the head store event this entry points at. Carried on
    // the entry so MaxAge GC doesn't depend on the head event still being
    // in staging. Defaults to empty on legacy kinographs (treated as
    // "unknown" and conservatively kept by GC).
    #[facet(default)]
    pub head_ts: String,
}

/// Commit-metadata line at the top of every root styxl blob.
///
/// `kind` is a fixed discriminator (`"root"`) that distinguishes headers
/// from entry lines — without it, a legacy header-less blob's first
/// entry would parse as a header since facet_styx tolerates unknown
/// fields. `id` is the lineage id (stable across versions). `parents`
/// lists the prior version's blob hashes (empty on genesis).
/// `ts`/`author`/`provenance` capture the commit that produced this
/// version.
#[derive(Facet, Debug, Clone, PartialEq, Eq)]
pub struct RootHeader {
    pub kind: String,
    pub id: String,
    #[facet(default)]
    pub parents: Vec<String>,
    #[facet(default)]
    pub ts: String,
    #[facet(default)]
    pub author: String,
    #[facet(default)]
    pub provenance: String,
}

impl Default for RootHeader {
    fn default() -> Self {
        Self {
            kind: HEADER_KIND.to_owned(),
            id: String::new(),
            parents: Vec::new(),
            ts: String::new(),
            author: String::new(),
            provenance: String::new(),
        }
    }
}

/// Fixed value of `RootHeader::kind`. Serves as the parse discriminator
/// that keeps entry lines from being read as headers on legacy blobs.
pub const HEADER_KIND: &str = "root";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootKinograph {
    pub header: RootHeader,
    pub entries: Vec<RootEntry>,
}

#[derive(Debug, thiserror::Error)]
pub enum RootError {
    #[error("failed to parse root kinograph: {0}")]
    Parse(String),
    #[error("failed to serialize root kinograph: {0}")]
    Serialize(String),
    #[error("invalid root entry [{idx}]: {reason}")]
    InvalidEntry { idx: usize, reason: String },
    #[error("duplicate id at entry [{idx}]: {id}")]
    DuplicateId { idx: usize, id: String },
    #[error("root kinograph is empty (missing header line)")]
    MissingHeader,
    #[error("root kinograph header has kind={kind:?}, expected {HEADER_KIND:?}")]
    WrongHeaderKind { kind: String },
}

impl RootEntry {
    /// Build a minimal root entry. Note and pin default to their
    /// empty/false forms; caller sets them explicitly if needed.
    /// `head_ts` is the RFC3339 timestamp of the head store event this
    /// entry points at — caller passes empty string for legacy/test cases
    /// where the ts isn't known.
    pub fn new(
        id: impl Into<String>,
        version: impl Into<String>,
        kind: impl Into<String>,
        metadata: BTreeMap<String, String>,
        head_ts: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            version: version.into(),
            kind: kind.into(),
            metadata,
            note: String::new(),
            pin: false,
            head_ts: head_ts.into(),
        }
    }

    pub fn note_opt(&self) -> Option<&str> {
        (!self.note.is_empty()).then_some(self.note.as_str())
    }
}

impl RootKinograph {
    /// Full constructor. Caller supplies the header (usually via
    /// [`Self::new_genesis`] or [`Self::new_child`]).
    pub fn new(header: RootHeader, entries: Vec<RootEntry>) -> Self {
        Self { header, entries }
    }

    /// Test/construction helper: build a kinograph with a default
    /// (empty) header. Production code paths that commit a new root
    /// version must use [`Self::new_genesis`] or [`Self::new_child`] so
    /// the lineage id + parent chain is populated.
    pub fn with_entries(entries: Vec<RootEntry>) -> Self {
        Self { header: RootHeader::default(), entries }
    }

    /// Compute the lineage id for a fresh root: hash of the entries-only
    /// byte stream (excluding the header line). Matches kino identity
    /// semantics — id = hash of the initial content payload.
    pub fn genesis_id(entries: &[RootEntry]) -> Result<String, RootError> {
        let body = serialize_entries_canonical(entries)?;
        Ok(Hash::of_content(body.as_bytes()).as_hex().to_owned())
    }

    /// Build the genesis version of a named root. Derives the lineage
    /// id from the entries-only payload; emits an empty `parents` list.
    pub fn new_genesis(
        entries: Vec<RootEntry>,
        ts: String,
        author: String,
        provenance: String,
    ) -> Result<Self, RootError> {
        let id = Self::genesis_id(&entries)?;
        let header = RootHeader {
            kind: HEADER_KIND.to_owned(),
            id,
            parents: vec![],
            ts,
            author,
            provenance,
        };
        Ok(Self { header, entries })
    }

    /// Build a non-genesis version, carrying the prior lineage id
    /// forward and recording the prior version's blob hash(es) as
    /// parents.
    pub fn new_child(
        prior_lineage_id: String,
        parent_blob_hashes: Vec<String>,
        entries: Vec<RootEntry>,
        ts: String,
        author: String,
        provenance: String,
    ) -> Self {
        let header = RootHeader {
            kind: HEADER_KIND.to_owned(),
            id: prior_lineage_id,
            parents: parent_blob_hashes,
            ts,
            author,
            provenance,
        };
        Self { header, entries }
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, RootError> {
        let s = std::str::from_utf8(bytes).map_err(|e| RootError::Parse(e.to_string()))?;
        Self::parse_str(s)
    }

    /// Parse a root-kinograph blob. Only the new header-first styxl
    /// form is accepted — legacy styx-wrapped and header-less blobs
    /// fail (see module-level docs on the hard cutover).
    pub fn parse_str(input: &str) -> Result<Self, RootError> {
        Self::parse_styxl(input)
    }

    /// Canonical styxl: line 1 is the header, then one entry per line
    /// (sorted by `id`, ascii-hex). Trailing LF after the last line.
    ///
    /// Uses `facet_styx::to_string_compact` so each struct emits as a
    /// single-line `{…}`; the default writer would unwrap into
    /// multi-line key-value pairs.
    pub fn to_styxl(&self) -> Result<String, RootError> {
        let mut out = facet_styx::to_string_compact(&self.header)
            .map_err(|e| RootError::Serialize(e.to_string()))?;
        out.push('\n');
        out.push_str(&serialize_entries_canonical(&self.entries)?);
        Ok(out)
    }

    /// Parse styxl. Line 1 must be a `RootHeader`; subsequent non-blank
    /// lines are `RootEntry`. Reports 1-based line numbers on parse
    /// failure. Rejects duplicate entry ids.
    pub fn parse_styxl(input: &str) -> Result<Self, RootError> {
        let mut lines = input.split('\n').enumerate();
        let header = loop {
            let (idx, raw) = lines.next().ok_or(RootError::MissingHeader)?;
            let line = raw.trim();
            if line.is_empty() {
                continue;
            }
            let h: RootHeader = facet_styx::from_str(line).map_err(|e| {
                RootError::Parse(format!("line {} (header): {e}", idx + 1))
            })?;
            if h.kind != HEADER_KIND {
                return Err(RootError::WrongHeaderKind { kind: h.kind });
            }
            break h;
        };

        let mut entries: Vec<RootEntry> = Vec::new();
        for (idx, raw) in lines {
            let line = raw.trim();
            if line.is_empty() {
                continue;
            }
            let entry: RootEntry = facet_styx::from_str(line).map_err(|e| {
                RootError::Parse(format!("line {}: {e}", idx + 1))
            })?;
            entries.push(entry);
        }
        validate_and_check_duplicates(&entries)?;
        Ok(Self { header, entries })
    }
}

/// Emit the entries-only payload: sorted by `id`, one entry per line,
/// trailing LF. Shared by `to_styxl` and `genesis_id` so the hash used
/// for the lineage id is byte-identical to what a fresh serialization
/// would reproduce.
fn serialize_entries_canonical(entries: &[RootEntry]) -> Result<String, RootError> {
    let mut canonical: Vec<RootEntry> = entries.to_vec();
    canonical.sort_by(|a, b| a.id.cmp(&b.id));
    let mut out = String::new();
    for entry in &canonical {
        let line = facet_styx::to_string_compact(entry)
            .map_err(|e| RootError::Serialize(e.to_string()))?;
        out.push_str(&line);
        out.push('\n');
    }
    Ok(out)
}

fn validate_and_check_duplicates(entries: &[RootEntry]) -> Result<(), RootError> {
    let mut seen: HashSet<String> = HashSet::new();
    for (idx, entry) in entries.iter().enumerate() {
        validate_entry(idx, entry)?;
        if !seen.insert(entry.id.clone()) {
            return Err(RootError::DuplicateId {
                idx,
                id: entry.id.clone(),
            });
        }
    }
    Ok(())
}

fn validate_entry(idx: usize, entry: &RootEntry) -> Result<(), RootError> {
    Hash::from_str(&entry.id).map_err(|e| RootError::InvalidEntry {
        idx,
        reason: format!("id is not a valid 64-hex hash: {e}"),
    })?;
    Hash::from_str(&entry.version).map_err(|e| RootError::InvalidEntry {
        idx,
        reason: format!("version is not a valid 64-hex hash: {e}"),
    })?;
    namespace::validate_kind(&entry.kind).map_err(|e: NamespaceError| {
        RootError::InvalidEntry {
            idx,
            reason: format!("invalid kind `{}`: {e}", entry.kind),
        }
    })?;
    for key in entry.metadata.keys() {
        namespace::validate_metadata_key(key).map_err(|e: NamespaceError| {
            RootError::InvalidEntry {
                idx,
                reason: format!("invalid metadata key `{key}`: {e}"),
            }
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u8) -> String {
        format!("{:02x}", n).repeat(32)
    }

    fn version_hash(n: u8) -> String {
        format!("{:02x}", n.wrapping_add(100)).repeat(32)
    }

    fn meta(name: &str) -> BTreeMap<String, String> {
        BTreeMap::from([("name".into(), name.into())])
    }

    fn sample_entry(n: u8) -> RootEntry {
        RootEntry::new(id(n), version_hash(n), "markdown", meta(&format!("name-{n}")), "")
    }

    #[test]
    fn root_is_reserved_kind() {
        assert!(namespace::validate_kind("root").is_ok());
    }

    #[test]
    fn root_kind_has_styxl_extension() {
        assert_eq!(namespace::ext_for_kind("root"), Some("styxl"));
    }

    #[test]
    fn roundtrip_minimal_root_entry() {
        let r = RootKinograph::with_entries(vec![sample_entry(1)]);
        let s = r.to_styxl().unwrap();
        let back = RootKinograph::parse_str(&s).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn roundtrip_with_note_and_pin() {
        let mut e = sample_entry(2);
        e.note = "genesis block".into();
        e.pin = true;
        let r = RootKinograph::with_entries(vec![e.clone()]);
        let s = r.to_styxl().unwrap();
        let back = RootKinograph::parse_str(&s).unwrap();
        assert_eq!(back.entries[0], e);
    }

    #[test]
    fn roundtrip_multiple_metadata_keys() {
        let mut e = sample_entry(3);
        e.metadata.insert("title".into(), "First Kino".into());
        e.metadata.insert("description".into(), "a brief note".into());
        e.metadata.insert("team::priority".into(), "high".into());
        let r = RootKinograph::with_entries(vec![e.clone()]);
        let s = r.to_styxl().unwrap();
        let back = RootKinograph::parse_str(&s).unwrap();
        assert_eq!(back.entries[0], e);
    }

    #[test]
    fn to_styxl_sorts_entries_by_id() {
        let r = RootKinograph::with_entries(vec![
            sample_entry(3),
            sample_entry(1),
            sample_entry(2),
        ]);
        let s = r.to_styxl().unwrap();
        let back = RootKinograph::parse_str(&s).unwrap();
        let ids: Vec<_> = back.entries.iter().map(|e| e.id.clone()).collect();
        assert_eq!(ids, vec![id(1), id(2), id(3)]);
    }

    #[test]
    fn duplicate_id_rejected_on_parse() {
        let r = RootKinograph::with_entries(vec![sample_entry(1), sample_entry(1)]);
        let s = r.to_styxl().unwrap();
        let err = RootKinograph::parse_str(&s).unwrap_err();
        assert!(matches!(err, RootError::DuplicateId { .. }), "got: {err:?}");
    }

    #[test]
    fn invalid_id_hash_rejected() {
        let mut e = sample_entry(1);
        e.id = "not-a-hash".into();
        let r = RootKinograph::with_entries(vec![e]);
        let s = r.to_styxl().unwrap();
        let err = RootKinograph::parse_str(&s).unwrap_err();
        assert!(matches!(err, RootError::InvalidEntry { .. }), "got: {err:?}");
    }

    #[test]
    fn invalid_version_hash_rejected() {
        let mut e = sample_entry(1);
        e.version = "not-a-hash".into();
        let r = RootKinograph::with_entries(vec![e]);
        let s = r.to_styxl().unwrap();
        let err = RootKinograph::parse_str(&s).unwrap_err();
        assert!(matches!(err, RootError::InvalidEntry { .. }), "got: {err:?}");
    }

    #[test]
    fn invalid_kind_rejected() {
        let mut e = sample_entry(1);
        e.kind = "random".into();
        let r = RootKinograph::with_entries(vec![e]);
        let s = r.to_styxl().unwrap();
        let err = RootKinograph::parse_str(&s).unwrap_err();
        assert!(matches!(err, RootError::InvalidEntry { .. }), "got: {err:?}");
    }

    #[test]
    fn invalid_metadata_key_rejected() {
        let mut e = sample_entry(1);
        e.metadata.insert("weird".into(), "v".into());
        let r = RootKinograph::with_entries(vec![e]);
        let s = r.to_styxl().unwrap();
        let err = RootKinograph::parse_str(&s).unwrap_err();
        assert!(matches!(err, RootError::InvalidEntry { .. }), "got: {err:?}");
    }

    #[test]
    fn namespaced_kind_accepted() {
        let mut e = sample_entry(1);
        e.kind = "team::diagram".into();
        let r = RootKinograph::with_entries(vec![e.clone()]);
        let s = r.to_styxl().unwrap();
        let back = RootKinograph::parse_str(&s).unwrap();
        assert_eq!(back.entries[0].kind, "team::diagram");
    }

    #[test]
    fn empty_root_parses() {
        let r = RootKinograph::with_entries(vec![]);
        let s = r.to_styxl().unwrap();
        let back = RootKinograph::parse_str(&s).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn canonical_output_is_byte_deterministic() {
        let a = RootKinograph::with_entries(vec![
            sample_entry(3),
            sample_entry(1),
            sample_entry(2),
        ]);
        let b = RootKinograph::with_entries(vec![
            sample_entry(1),
            sample_entry(2),
            sample_entry(3),
        ]);
        assert_eq!(a.to_styxl().unwrap(), b.to_styxl().unwrap());
    }

    #[test]
    fn to_styxl_emits_metadata_keys_in_sorted_order() {
        let mut e = sample_entry(1);
        e.metadata.clear();
        e.metadata.insert("title".into(), "Z".into());
        e.metadata.insert("description".into(), "M".into());
        e.metadata.insert("name".into(), "A".into());
        let r = RootKinograph::with_entries(vec![e]);
        let s = r.to_styxl().unwrap();
        let desc_pos = s.find("description").expect("description key");
        let name_pos = s.find("name").expect("name key");
        let title_pos = s.find("title").expect("title key");
        assert!(
            desc_pos < name_pos && name_pos < title_pos,
            "metadata keys not in sorted order in:\n{s}"
        );
    }

    #[test]
    fn parse_from_bytes_matches_parse_str() {
        let r = RootKinograph::with_entries(vec![sample_entry(1)]);
        let s = r.to_styxl().unwrap();
        let from_str = RootKinograph::parse_str(&s).unwrap();
        let from_bytes = RootKinograph::parse(s.as_bytes()).unwrap();
        assert_eq!(from_str, from_bytes);
    }

    #[test]
    fn parse_invalid_utf8_errors_as_parse() {
        let bad: &[u8] = &[0xff, 0xfe, 0xfd];
        let err = RootKinograph::parse(bad).unwrap_err();
        assert!(matches!(err, RootError::Parse(_)), "got: {err:?}");
    }

    #[test]
    fn note_opt_treats_empty_as_none() {
        let e = sample_entry(1);
        assert_eq!(e.note_opt(), None);
        let mut e2 = sample_entry(1);
        e2.note = "hi".into();
        assert_eq!(e2.note_opt(), Some("hi"));
    }

    #[test]
    fn styxl_roundtrip_preserves_entries() {
        let mut e = sample_entry(1);
        e.note = "origin".into();
        e.pin = true;
        let r = RootKinograph::with_entries(vec![e, sample_entry(2)]);
        let s = r.to_styxl().unwrap();
        let back = RootKinograph::parse_styxl(&s).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn styxl_rejects_duplicate_ids_on_parse() {
        // Hand-craft duplicate entry lines since `to_styxl` can't produce
        // duplicates. Header line is also hand-crafted (empty parents).
        let header_line = {
            let r = RootKinograph::with_entries(vec![]);
            r.to_styxl().unwrap().trim_end().to_owned()
        };
        let entry_line = {
            let r = RootKinograph::with_entries(vec![sample_entry(1)]);
            // Skip the header line to get just the entry.
            r.to_styxl().unwrap().lines().nth(1).unwrap().to_owned()
        };
        let malformed = format!("{header_line}\n{entry_line}\n{entry_line}\n");
        let err = RootKinograph::parse_styxl(&malformed).unwrap_err();
        assert!(matches!(err, RootError::DuplicateId { .. }), "got: {err:?}");
    }

    #[test]
    fn styxl_reports_line_number_on_parse_error() {
        let good = RootKinograph::with_entries(vec![sample_entry(1)])
            .to_styxl()
            .unwrap();
        let input = format!("{good}{{garbage}}\n");
        let err = RootKinograph::parse_styxl(&input).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("line 3"), "error should cite line 3 (header + 1 entry + garbage), got: {msg}");
    }

    #[test]
    fn root_kind_maps_to_styxl_extension() {
        assert_eq!(namespace::ext_for_kind("root"), Some("styxl"));
        assert_eq!(namespace::ext_for_kind("kinograph"), Some("styxl"));
    }

    // ------------------------------------------------------------------
    // et1t: header-first styxl format (self-contained commit metadata)
    // ------------------------------------------------------------------

    fn hash_hex(n: u8) -> String {
        format!("{:02x}", n.wrapping_add(200)).repeat(32)
    }

    #[test]
    fn header_roundtrip_with_entries() {
        let header = RootHeader {
            kind: HEADER_KIND.to_owned(),
            id: hash_hex(1),
            parents: vec![hash_hex(2)],
            ts: "2026-04-21T12:00:00Z".into(),
            author: "YJ".into(),
            provenance: "commit".into(),
        };
        let r = RootKinograph::new(header.clone(), vec![sample_entry(1), sample_entry(2)]);
        let s = r.to_styxl().unwrap();
        let back = RootKinograph::parse_styxl(&s).unwrap();
        assert_eq!(back.header, header);
        assert_eq!(back.entries.len(), 2);
    }

    #[test]
    fn genesis_parents_empty_and_roundtrips() {
        let r = RootKinograph::new_genesis(
            vec![sample_entry(1)],
            "2026-04-21T12:00:00Z".into(),
            "YJ".into(),
            "commit".into(),
        )
        .unwrap();
        assert!(r.header.parents.is_empty(), "genesis must have empty parents");
        assert!(!r.header.id.is_empty(), "genesis must derive an id");

        let s = r.to_styxl().unwrap();
        let back = RootKinograph::parse_styxl(&s).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn genesis_id_equals_hash_of_entries_only_bytes() {
        let entries = vec![sample_entry(1), sample_entry(2)];
        let id = RootKinograph::genesis_id(&entries).unwrap();

        // Hand-compute: same canonical entry serialization, hashed.
        let body = serialize_entries_canonical(&entries).unwrap();
        let expected = Hash::of_content(body.as_bytes()).as_hex().to_owned();
        assert_eq!(id, expected);
    }

    #[test]
    fn child_carries_prior_lineage_id() {
        let genesis = RootKinograph::new_genesis(
            vec![sample_entry(1)],
            "2026-04-21T12:00:00Z".into(),
            "YJ".into(),
            "commit".into(),
        )
        .unwrap();
        let parent_blob_hash = hash_hex(9); // simulated
        let child = RootKinograph::new_child(
            genesis.header.id.clone(),
            vec![parent_blob_hash.clone()],
            vec![sample_entry(1), sample_entry(2)],
            "2026-04-21T12:01:00Z".into(),
            "YJ".into(),
            "commit".into(),
        );
        assert_eq!(child.header.id, genesis.header.id, "lineage id must carry forward");
        assert_eq!(child.header.parents, vec![parent_blob_hash]);
    }

    #[test]
    fn multiple_parents_roundtrip() {
        // Merge commits produce multi-parent headers. Guard the
        // serializer/parser against that shape.
        let header = RootHeader {
            kind: HEADER_KIND.to_owned(),
            id: hash_hex(1),
            parents: vec![hash_hex(2), hash_hex(3), hash_hex(4)],
            ts: "2026-04-21T12:00:00Z".into(),
            author: "YJ".into(),
            provenance: "merge".into(),
        };
        let r = RootKinograph::new(header.clone(), vec![]);
        let s = r.to_styxl().unwrap();
        let back = RootKinograph::parse_styxl(&s).unwrap();
        assert_eq!(back.header.parents, header.parents);
    }

    #[test]
    fn header_line_comes_first() {
        let r = RootKinograph::new_genesis(
            vec![sample_entry(1), sample_entry(2)],
            "2026-04-21T12:00:00Z".into(),
            "YJ".into(),
            "commit".into(),
        )
        .unwrap();
        let s = r.to_styxl().unwrap();
        let mut lines = s.lines();
        let first = lines.next().expect("has first line");
        // Header has `parents` and `ts`; entries have `version`.
        assert!(first.contains("parents"), "header line should contain parents: {first}");
        assert!(!first.contains("version"), "header line shouldn't be an entry: {first}");
    }

    #[test]
    fn parse_rejects_empty_input_as_missing_header() {
        let err = RootKinograph::parse_styxl("").unwrap_err();
        assert!(matches!(err, RootError::MissingHeader), "got: {err:?}");
    }

    #[test]
    fn parse_rejects_header_less_blob_via_kind_discriminator() {
        // Legacy header-less blob: line 1 is an entry. Its `kind` is
        // `markdown`/`kinograph`/etc., not `root` — the discriminator
        // check rejects it instead of silently reading the entry as a
        // partial header.
        let entry_line = facet_styx::to_string_compact(&sample_entry(1)).unwrap();
        let err = RootKinograph::parse_styxl(&format!("{entry_line}\n")).unwrap_err();
        assert!(
            matches!(err, RootError::WrongHeaderKind { .. }),
            "expected WrongHeaderKind, got: {err:?}",
        );
    }
}
