//! Root kinograph: a `kind: root` content blob. Its styx document is a
//! top-level `entries (…)` list where each entry inlines the full leaf
//! view of one owned kino — `id`, the pinned `version` content hash, the
//! kino's `kind`, and its authoritative `metadata`. Optional `note` and
//! `pin` carry composition hints.
//!
//! Root kinographs differ structurally from composition kinographs
//! (`crate::kinograph::Kinograph`): composition entries are pure
//! `{id, name?, pin?, note?}` pointers, root entries are the metadata
//! home. The parser/serializer lives in this separate module so the
//! field sets don't leak between the two shapes.
//!
//! Canonical form: entries sorted by `id` (ascii-hex), metadata keys
//! sorted (`BTreeMap` handles this). `to_styx` always emits canonical.

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
}

#[derive(Facet, Debug, Clone, PartialEq, Eq)]
pub struct RootKinograph {
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
}

impl RootEntry {
    /// Build a minimal root entry. Note and pin default to their
    /// empty/false forms; caller sets them explicitly if needed.
    pub fn new(
        id: impl Into<String>,
        version: impl Into<String>,
        kind: impl Into<String>,
        metadata: BTreeMap<String, String>,
    ) -> Self {
        Self {
            id: id.into(),
            version: version.into(),
            kind: kind.into(),
            metadata,
            note: String::new(),
            pin: false,
        }
    }

    pub fn note_opt(&self) -> Option<&str> {
        (!self.note.is_empty()).then_some(self.note.as_str())
    }
}

impl RootKinograph {
    pub fn parse(bytes: &[u8]) -> Result<Self, RootError> {
        let s = std::str::from_utf8(bytes).map_err(|e| RootError::Parse(e.to_string()))?;
        Self::parse_str(s)
    }

    pub fn parse_str(input: &str) -> Result<Self, RootError> {
        let parsed: RootKinograph = facet_styx::from_str(input)
            .map_err(|e| RootError::Parse(e.to_string()))?;
        let mut seen: HashSet<String> = HashSet::new();
        for (idx, entry) in parsed.entries.iter().enumerate() {
            validate_entry(idx, entry)?;
            if !seen.insert(entry.id.clone()) {
                return Err(RootError::DuplicateId {
                    idx,
                    id: entry.id.clone(),
                });
            }
        }
        Ok(parsed)
    }

    /// Emit canonical styx: entries sorted by `id` (ascii-hex ordering).
    /// Metadata keys are already sorted via `BTreeMap`.
    pub fn to_styx(&self) -> Result<String, RootError> {
        let mut canonical = self.clone();
        canonical.entries.sort_by(|a, b| a.id.cmp(&b.id));
        facet_styx::to_string(&canonical).map_err(|e| RootError::Serialize(e.to_string()))
    }
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
        RootEntry::new(id(n), version_hash(n), "markdown", meta(&format!("name-{n}")))
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
        let r = RootKinograph {
            entries: vec![sample_entry(1)],
        };
        let s = r.to_styx().unwrap();
        let back = RootKinograph::parse_str(&s).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn roundtrip_with_note_and_pin() {
        let mut e = sample_entry(2);
        e.note = "genesis block".into();
        e.pin = true;
        let r = RootKinograph {
            entries: vec![e.clone()],
        };
        let s = r.to_styx().unwrap();
        let back = RootKinograph::parse_str(&s).unwrap();
        assert_eq!(back.entries[0], e);
    }

    #[test]
    fn roundtrip_multiple_metadata_keys() {
        let mut e = sample_entry(3);
        e.metadata.insert("title".into(), "First Kino".into());
        e.metadata.insert("description".into(), "a brief note".into());
        e.metadata.insert("team::priority".into(), "high".into());
        let r = RootKinograph {
            entries: vec![e.clone()],
        };
        let s = r.to_styx().unwrap();
        let back = RootKinograph::parse_str(&s).unwrap();
        assert_eq!(back.entries[0], e);
    }

    #[test]
    fn to_styx_sorts_entries_by_id() {
        let r = RootKinograph {
            entries: vec![sample_entry(3), sample_entry(1), sample_entry(2)],
        };
        let s = r.to_styx().unwrap();
        let back = RootKinograph::parse_str(&s).unwrap();
        let ids: Vec<_> = back.entries.iter().map(|e| e.id.clone()).collect();
        assert_eq!(ids, vec![id(1), id(2), id(3)]);
    }

    #[test]
    fn duplicate_id_rejected_on_parse() {
        let r = RootKinograph {
            entries: vec![sample_entry(1), sample_entry(1)],
        };
        let s = r.to_styx().unwrap();
        let err = RootKinograph::parse_str(&s).unwrap_err();
        assert!(matches!(err, RootError::DuplicateId { .. }), "got: {err:?}");
    }

    #[test]
    fn invalid_id_hash_rejected() {
        let mut e = sample_entry(1);
        e.id = "not-a-hash".into();
        let r = RootKinograph {
            entries: vec![e],
        };
        let s = r.to_styx().unwrap();
        let err = RootKinograph::parse_str(&s).unwrap_err();
        assert!(matches!(err, RootError::InvalidEntry { .. }), "got: {err:?}");
    }

    #[test]
    fn invalid_version_hash_rejected() {
        let mut e = sample_entry(1);
        e.version = "not-a-hash".into();
        let r = RootKinograph {
            entries: vec![e],
        };
        let s = r.to_styx().unwrap();
        let err = RootKinograph::parse_str(&s).unwrap_err();
        assert!(matches!(err, RootError::InvalidEntry { .. }), "got: {err:?}");
    }

    #[test]
    fn invalid_kind_rejected() {
        let mut e = sample_entry(1);
        e.kind = "random".into();
        let r = RootKinograph {
            entries: vec![e],
        };
        let s = r.to_styx().unwrap();
        let err = RootKinograph::parse_str(&s).unwrap_err();
        assert!(matches!(err, RootError::InvalidEntry { .. }), "got: {err:?}");
    }

    #[test]
    fn invalid_metadata_key_rejected() {
        let mut e = sample_entry(1);
        e.metadata.insert("weird".into(), "v".into());
        let r = RootKinograph {
            entries: vec![e],
        };
        let s = r.to_styx().unwrap();
        let err = RootKinograph::parse_str(&s).unwrap_err();
        assert!(matches!(err, RootError::InvalidEntry { .. }), "got: {err:?}");
    }

    #[test]
    fn namespaced_kind_accepted() {
        let mut e = sample_entry(1);
        e.kind = "team::diagram".into();
        let r = RootKinograph {
            entries: vec![e.clone()],
        };
        let s = r.to_styx().unwrap();
        let back = RootKinograph::parse_str(&s).unwrap();
        assert_eq!(back.entries[0].kind, "team::diagram");
    }

    #[test]
    fn empty_root_parses() {
        let r = RootKinograph { entries: vec![] };
        let s = r.to_styx().unwrap();
        let back = RootKinograph::parse_str(&s).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn canonical_output_is_byte_deterministic() {
        // Same logical content produced from different insertion orders
        // must serialize to byte-identical styx.
        let a = RootKinograph {
            entries: vec![sample_entry(3), sample_entry(1), sample_entry(2)],
        };
        let b = RootKinograph {
            entries: vec![sample_entry(1), sample_entry(2), sample_entry(3)],
        };
        assert_eq!(a.to_styx().unwrap(), b.to_styx().unwrap());
    }

    #[test]
    fn to_styx_emits_metadata_keys_in_sorted_order() {
        // The canonical-form rule says metadata keys are sorted. BTreeMap
        // gives us this for free today — this test pins the invariant so
        // future refactors (e.g. switching the struct to HashMap) fail
        // loudly instead of silently breaking determinism.
        let mut e = sample_entry(1);
        e.metadata.clear();
        e.metadata.insert("title".into(), "Z".into());
        e.metadata.insert("description".into(), "M".into());
        e.metadata.insert("name".into(), "A".into());
        let r = RootKinograph {
            entries: vec![e],
        };
        let s = r.to_styx().unwrap();
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
        let r = RootKinograph {
            entries: vec![sample_entry(1)],
        };
        let s = r.to_styx().unwrap();
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
    fn missing_pin_and_note_default_to_false_and_empty() {
        // Parsing a styx document that omits optional `note` and `pin`
        // must yield `pin == false` and `note.is_empty()`. Guards the
        // `#[facet(default)]` contract from silent regression.
        let id = id(1);
        let version = version_hash(1);
        let input = format!(
            "entries ({{id {id}, version {version}, kind markdown, metadata {{name x}}}})"
        );
        let r = RootKinograph::parse_str(&input).unwrap();
        assert_eq!(r.entries.len(), 1);
        assert!(!r.entries[0].pin, "pin should default to false");
        assert!(r.entries[0].note.is_empty(), "note should default to empty");
    }

    #[test]
    fn note_opt_treats_empty_as_none() {
        let e = sample_entry(1);
        assert_eq!(e.note_opt(), None);
        let mut e2 = sample_entry(1);
        e2.note = "hi".into();
        assert_eq!(e2.note_opt(), Some("hi"));
    }

    // ------------------------------------------------------------------
    // tx3e: styxl (one-entry-per-line) format for root kinographs
    // ------------------------------------------------------------------

    #[test]
    fn to_styxl_emits_one_line_per_entry() {
        let r = RootKinograph {
            entries: vec![sample_entry(1), sample_entry(2), sample_entry(3)],
        };
        let s = r.to_styxl().unwrap();
        let lines: Vec<_> = s.lines().collect();
        assert_eq!(lines.len(), 3, "got: {s:?}");
        for line in &lines {
            assert!(line.starts_with('{'), "line did not start with '{{': {line:?}");
            assert!(line.ends_with('}'), "line did not end with '}}': {line:?}");
        }
    }

    #[test]
    fn styxl_roundtrip_preserves_entries() {
        let mut e = sample_entry(1);
        e.note = "origin".into();
        e.pin = true;
        let r = RootKinograph {
            entries: vec![e, sample_entry(2)],
        };
        let s = r.to_styxl().unwrap();
        let back = RootKinograph::parse_styxl(&s).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn styxl_sorts_entries_by_id_canonically() {
        // `to_styxl` emits the canonical sort order (by id) just like the
        // existing `to_styx`. Guards against byte drift across machines.
        let r = RootKinograph {
            entries: vec![sample_entry(3), sample_entry(1), sample_entry(2)],
        };
        let s = r.to_styxl().unwrap();
        let first_ids: Vec<_> = s
            .lines()
            .map(|l| {
                // extract the id hex right after `{id `
                let rest = l.strip_prefix("{id ").expect("line starts with {id ");
                rest.split(|c: char| c == ',' || c == '}').next().unwrap().trim().to_owned()
            })
            .collect();
        assert_eq!(first_ids, vec![id(1), id(2), id(3)]);
    }

    #[test]
    fn styxl_empty_root_serializes_to_empty_string() {
        let r = RootKinograph { entries: vec![] };
        let s = r.to_styxl().unwrap();
        assert!(s.is_empty(), "empty root should produce empty styxl: {s:?}");
        let back = RootKinograph::parse_styxl("").unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn styxl_rejects_duplicate_ids_on_parse() {
        let r = RootKinograph {
            entries: vec![sample_entry(1), sample_entry(1)],
        };
        // Bypass `to_styxl`'s canonicalization by hand-crafting the
        // duplicate; writer wouldn't normally emit duplicates but the
        // parser still must reject them as a safety net.
        let line = {
            let single = RootKinograph {
                entries: vec![sample_entry(1)],
            };
            single.to_styxl().unwrap().trim_end().to_owned()
        };
        let malformed = format!("{line}\n{line}\n");
        let err = RootKinograph::parse_styxl(&malformed).unwrap_err();
        assert!(matches!(err, RootError::DuplicateId { .. }), "got: {err:?}");
        let _ = r;
    }

    #[test]
    fn styxl_reports_line_number_on_parse_error() {
        let good = RootKinograph {
            entries: vec![sample_entry(1)],
        }
        .to_styxl()
        .unwrap();
        let input = format!("{good}{{garbage}}\n");
        let err = RootKinograph::parse_styxl(&input).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("line 2"), "error should cite line 2, got: {msg}");
    }

    #[test]
    fn parse_accepts_legacy_styx_wrapped_form() {
        // Old `entries (...)` form must still load so repos holding
        // pre-reformat root blobs remain readable.
        let r = RootKinograph {
            entries: vec![sample_entry(1)],
        };
        let legacy = facet_styx::to_string(&r).unwrap();
        let back = RootKinograph::parse_str(&legacy).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn parse_auto_detects_styxl_form() {
        let r = RootKinograph {
            entries: vec![sample_entry(1)],
        };
        let styxl = r.to_styxl().unwrap();
        let back = RootKinograph::parse_str(&styxl).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn root_kind_maps_to_styxl_extension() {
        // Extension follows the wire format; switching to styxl implies
        // the store filename is `<hash>.styxl`.
        assert_eq!(namespace::ext_for_kind("root"), Some("styxl"));
        assert_eq!(namespace::ext_for_kind("kinograph"), Some("styxl"));
    }
}
