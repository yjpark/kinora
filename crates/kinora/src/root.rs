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

#[derive(Debug)]
pub enum RootError {
    Parse(String),
    Serialize(String),
    InvalidEntry { idx: usize, reason: String },
    DuplicateId { idx: usize, id: String },
    Utf8(std::string::FromUtf8Error),
}

impl std::fmt::Display for RootError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RootError::Parse(m) => write!(f, "failed to parse root kinograph: {m}"),
            RootError::Serialize(m) => write!(f, "failed to serialize root kinograph: {m}"),
            RootError::InvalidEntry { idx, reason } => {
                write!(f, "invalid root entry [{idx}]: {reason}")
            }
            RootError::DuplicateId { idx, id } => {
                write!(f, "duplicate id at entry [{idx}]: {id}")
            }
            RootError::Utf8(e) => write!(f, "root kinograph bytes are not valid UTF-8: {e}"),
        }
    }
}

impl std::error::Error for RootError {}

impl From<std::string::FromUtf8Error> for RootError {
    fn from(e: std::string::FromUtf8Error) -> Self {
        RootError::Utf8(e)
    }
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
    fn root_kind_has_styx_extension() {
        assert_eq!(namespace::ext_for_kind("root"), Some("styx"));
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
    fn note_opt_treats_empty_as_none() {
        let e = sample_entry(1);
        assert_eq!(e.note_opt(), None);
        let mut e2 = sample_entry(1);
        e2.note = "hi".into();
        assert_eq!(e2.note_opt(), Some("hi"));
    }
}
