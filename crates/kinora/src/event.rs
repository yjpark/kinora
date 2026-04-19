use std::collections::BTreeMap;

use facet::Facet;

use crate::hash::Hash;

/// Append-only ledger event.
///
/// Fields stored as String for facet-json round-trip simplicity in MVP.
/// Typed accessors (`id_hash`, `content_hash`, `parent_hashes`) parse on demand.
/// Metadata values are strings in MVP — structured values (booleans, arrays)
/// can be serialized as JSON-encoded strings by the caller; richer typing is
/// deferred post-bootstrap.
#[derive(Facet, Debug, Clone, PartialEq)]
pub struct Event {
    pub kind: String,
    pub id: String,
    pub hash: String,
    pub parents: Vec<String>,
    pub ts: String,
    pub author: String,
    pub provenance: String,
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug)]
pub enum EventError {
    Serialize(String),
    Parse(String),
}

impl std::fmt::Display for EventError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EventError::Serialize(m) => write!(f, "event serialize error: {m}"),
            EventError::Parse(m) => write!(f, "event parse error: {m}"),
        }
    }
}

impl std::error::Error for EventError {}

impl Event {
    pub fn to_json_line(&self) -> Result<String, EventError> {
        let s =
            facet_json::to_string(self).map_err(|e| EventError::Serialize(e.to_string()))?;
        Ok(s)
    }

    pub fn from_json_line(line: &str) -> Result<Self, EventError> {
        facet_json::from_str(line).map_err(|e| EventError::Parse(e.to_string()))
    }

    pub fn is_birth(&self) -> bool {
        self.id == self.hash && self.parents.is_empty()
    }

    /// Content-addressed identifier for this event.
    ///
    /// Computed as `BLAKE3(canonical-json-line)`. `to_json_line()` is the
    /// canonical encoding: `BTreeMap` iterates keys in sorted order, `Vec`
    /// preserves order, and facet-json writes struct fields in declaration
    /// order — so the same logical event always produces the same hash,
    /// enabling dedup across branches and immutable one-file-per-event
    /// storage in `.kinora/hot/`.
    pub fn event_hash(&self) -> Result<Hash, EventError> {
        let line = self.to_json_line()?;
        Ok(Hash::of_content(line.as_bytes()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_event() -> Event {
        let content = b"hello kinora";
        let h = Hash::of_content(content);
        Event {
            kind: "markdown".into(),
            id: h.as_hex().into(),
            hash: h.as_hex().into(),
            parents: vec![],
            ts: "2026-04-18T09:20:00Z".into(),
            author: "yj".into(),
            provenance: "test".into(),
            metadata: {
                let mut m = BTreeMap::new();
                m.insert("name".into(), "greeting".into());
                m
            },
        }
    }

    #[test]
    fn roundtrip_via_json_line() {
        let e = sample_event();
        let line = e.to_json_line().unwrap();
        let back = Event::from_json_line(&line).unwrap();
        assert_eq!(back, e);
    }

    #[test]
    fn json_line_has_no_newlines() {
        let e = sample_event();
        let line = e.to_json_line().unwrap();
        assert!(!line.contains('\n'), "json line contained newline: {line}");
    }

    #[test]
    fn json_line_contains_expected_fields() {
        let e = sample_event();
        let line = e.to_json_line().unwrap();
        for key in ["kind", "id", "hash", "parents", "ts", "author", "provenance", "metadata"] {
            assert!(line.contains(key), "missing {key} in {line}");
        }
    }

    #[test]
    fn birth_event_detected() {
        let e = sample_event();
        assert!(e.is_birth());
    }

    #[test]
    fn version_event_not_birth() {
        let mut e = sample_event();
        let new_hash = Hash::of_content(b"v2").as_hex().to_owned();
        e.parents = vec![e.hash.clone()];
        e.hash = new_hash;
        assert!(!e.is_birth());
    }

    #[test]
    fn event_hash_is_deterministic_across_calls() {
        let e = sample_event();
        let a = e.event_hash().unwrap();
        let b = e.event_hash().unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn event_hash_is_blake3_of_json_line() {
        let e = sample_event();
        let expected = Hash::of_content(e.to_json_line().unwrap().as_bytes());
        assert_eq!(e.event_hash().unwrap(), expected);
    }

    #[test]
    fn event_hash_differs_when_metadata_differs() {
        let a = sample_event();
        let mut b = sample_event();
        b.metadata.insert("title".into(), "bonjour".into());
        assert_ne!(a.event_hash().unwrap(), b.event_hash().unwrap());
    }

    #[test]
    fn event_hash_is_stable_across_metadata_insertion_order() {
        // BTreeMap iterates keys in sorted order so insertion order can't
        // change the hash. Verifies the determinism invariant of
        // `event_hash`.
        let mut a = sample_event();
        a.metadata.clear();
        a.metadata.insert("name".into(), "n".into());
        a.metadata.insert("title".into(), "t".into());

        let mut b = sample_event();
        b.metadata.clear();
        b.metadata.insert("title".into(), "t".into());
        b.metadata.insert("name".into(), "n".into());

        assert_eq!(a.event_hash().unwrap(), b.event_hash().unwrap());
    }
}
