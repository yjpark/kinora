use std::collections::BTreeMap;

use facet::Facet;

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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::Hash;

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
}
