use std::collections::BTreeMap;

use facet::Facet;

use crate::hash::Hash;

/// Canonical value of `Event::event_kind` for content-store events.
pub const EVENT_KIND_STORE: &str = "store";

/// Canonical value of `Event::event_kind` for kino-to-root assign events
/// (phase 3). Assign events carry no content — they name a kino id and
/// the root it should belong to, optionally superseding prior assigns.
pub const EVENT_KIND_ASSIGN: &str = "assign";

/// Append-only ledger event.
///
/// `event_kind` is the top-level discriminator introduced in phase 3 — it
/// distinguishes content-store events from non-store events (e.g. `assign`,
/// metadata resolutions). Consumers that operate on content (resolver,
/// render, compact) filter on `is_store_event()` so non-store events slide
/// past harmlessly until their dedicated handler lands.
///
/// Fields stored as String for facet-json round-trip simplicity in MVP.
/// Typed accessors (`id_hash`, `content_hash`, `parent_hashes`) parse on demand.
/// Metadata values are strings in MVP — structured values (booleans, arrays)
/// can be serialized as JSON-encoded strings by the caller; richer typing is
/// deferred post-bootstrap.
#[derive(Facet, Debug, Clone, PartialEq)]
pub struct Event {
    pub event_kind: String,
    pub kind: String,
    pub id: String,
    pub hash: String,
    pub parents: Vec<String>,
    pub ts: String,
    pub author: String,
    pub provenance: String,
    pub metadata: BTreeMap<String, String>,
}

/// Pre-phase-3 on-disk event shape, for backward-compat parsing of
/// legacy hot files that were written before the `event_kind`
/// discriminator landed. Promoted to `Event` with `event_kind = "store"`.
#[derive(Facet, Debug, Clone, PartialEq)]
struct LegacyEvent {
    kind: String,
    id: String,
    hash: String,
    parents: Vec<String>,
    ts: String,
    author: String,
    provenance: String,
    metadata: BTreeMap<String, String>,
}

impl From<LegacyEvent> for Event {
    fn from(l: LegacyEvent) -> Self {
        Event {
            event_kind: EVENT_KIND_STORE.to_owned(),
            kind: l.kind,
            id: l.id,
            hash: l.hash,
            parents: l.parents,
            ts: l.ts,
            author: l.author,
            provenance: l.provenance,
            metadata: l.metadata,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EventError {
    #[error("event serialize error: {0}")]
    Serialize(String),
    #[error("event parse error: {0}")]
    Parse(String),
}

impl Event {
    /// Construct a store-kind event. Syntactic sugar so call sites don't
    /// have to repeat `event_kind: EVENT_KIND_STORE.into()`.
    #[allow(clippy::too_many_arguments)]
    pub fn new_store(
        kind: String,
        id: String,
        hash: String,
        parents: Vec<String>,
        ts: String,
        author: String,
        provenance: String,
        metadata: BTreeMap<String, String>,
    ) -> Self {
        Event {
            event_kind: EVENT_KIND_STORE.to_owned(),
            kind,
            id,
            hash,
            parents,
            ts,
            author,
            provenance,
            metadata,
        }
    }

    /// True iff this event belongs to the content-store track (the only
    /// event kind consumers like resolver/render/compact should follow).
    pub fn is_store_event(&self) -> bool {
        self.event_kind == EVENT_KIND_STORE
    }

    pub fn to_json_line(&self) -> Result<String, EventError> {
        let s =
            facet_json::to_string(self).map_err(|e| EventError::Serialize(e.to_string()))?;
        Ok(s)
    }

    /// Parse a single ledger JSON line.
    ///
    /// New-shape events (with `event_kind`) parse directly. Pre-phase-3
    /// lines (no `event_kind`) fall back to `LegacyEvent` and promote with
    /// `event_kind = "store"`. The fallback is triggered **only** when the
    /// primary parse fails specifically because `event_kind` is missing —
    /// a new-shape line with other errors (wrong type on some field, bad
    /// JSON) surfaces its real error rather than being masked by a
    /// spurious legacy promotion.
    ///
    /// Identity semantics: a legacy event's on-disk filename is derived
    /// from its *legacy* canonical form (no `event_kind`), but
    /// `Event::event_hash()` after promotion uses the *new* canonical
    /// form. Re-hashing a promoted legacy event therefore yields a
    /// different hash than its on-disk filename — the invariant
    /// `file_path == event_hash` only holds for events written by
    /// `Ledger::write_event` after the phase-3 cutover.
    pub fn from_json_line(line: &str) -> Result<Self, EventError> {
        match facet_json::from_str::<Self>(line) {
            Ok(e) => Ok(e),
            Err(primary) => {
                let msg = primary.to_string();
                if msg.contains("missing field `event_kind`") {
                    match facet_json::from_str::<LegacyEvent>(line) {
                        Ok(legacy) => Ok(Event::from(legacy)),
                        Err(_) => Err(EventError::Parse(msg)),
                    }
                } else {
                    Err(EventError::Parse(msg))
                }
            }
        }
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
        Event::new_store(
            "markdown".into(),
            h.as_hex().into(),
            h.as_hex().into(),
            vec![],
            "2026-04-18T09:20:00Z".into(),
            "yj".into(),
            "test".into(),
            BTreeMap::from([("name".to_string(), "greeting".to_string())]),
        )
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
        for key in [
            "event_kind",
            "kind",
            "id",
            "hash",
            "parents",
            "ts",
            "author",
            "provenance",
            "metadata",
        ] {
            assert!(line.contains(key), "missing {key} in {line}");
        }
    }

    #[test]
    fn new_store_sets_event_kind_to_store() {
        let e = sample_event();
        assert_eq!(e.event_kind, EVENT_KIND_STORE);
        assert!(e.is_store_event());
    }

    #[test]
    fn non_store_event_kind_is_not_is_store_event() {
        let mut e = sample_event();
        e.event_kind = "assign".into();
        assert!(!e.is_store_event());
    }

    #[test]
    fn malformed_new_shape_event_does_not_fall_back_to_legacy() {
        // A line that *has* `event_kind` but is otherwise malformed (wrong
        // type on some other field) must surface its real parse error
        // rather than silently promote to a store event.
        let bad = r#"{"event_kind":"assign","kind":"markdown","id":"aa","hash":"aa","parents":[],"ts":"2026-04-18T09:20:00Z","author":"yj","provenance":"test","metadata":"not-a-map"}"#;
        let err = Event::from_json_line(bad).unwrap_err();
        let msg = match err {
            EventError::Parse(m) => m,
            other => panic!("expected Parse, got {other:?}"),
        };
        assert!(
            !msg.contains("missing field `event_kind`"),
            "primary error should not be a missing-field error: {msg}"
        );
    }

    #[test]
    fn legacy_event_line_parses_as_store_event() {
        // A JSON line written by pre-phase-3 code — no `event_kind` field.
        // from_json_line must accept it and materialize as a store event.
        let legacy = r#"{"kind":"markdown","id":"aa","hash":"aa","parents":[],"ts":"2026-04-18T09:20:00Z","author":"yj","provenance":"test","metadata":{"name":"greeting"}}"#;
        let got = Event::from_json_line(legacy).unwrap();
        assert!(got.is_store_event(), "legacy event must parse as store");
        assert_eq!(got.kind, "markdown");
        assert_eq!(got.id, "aa");
        assert_eq!(got.metadata.get("name").map(String::as_str), Some("greeting"));
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

    #[test]
    fn event_kind_differs_from_content_kind() {
        // `event_kind` discriminates the event track; `kind` is the content
        // blob kind. They live independently — a store-track event can
        // carry any content kind.
        let e = sample_event();
        assert_eq!(e.event_kind, "store");
        assert_eq!(e.kind, "markdown");
    }
}
