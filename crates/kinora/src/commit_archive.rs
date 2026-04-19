//! Per-commit archive blob format for the `commits` root (xi21 / kinora-q6bo).
//!
//! An archive captures the ordered list of staged events consumed by a
//! single root's commit, so `.kinora/staged/` can be pruned without
//! losing provenance. Blobs are content-addressed via `ContentStore`
//! like any other kino content.
//!
//! ## Wire format
//!
//! UTF-8 JSONL. The first line is a schema header; each subsequent line
//! is one `Event`'s `to_json_line()` output. Lines are LF-separated.
//! A trailing LF after the final event line is written but optional on
//! parse.
//!
//! ```text
//! {"@schema":"kinora-commit-archive-v1"}
//! {"event_kind":"store",...}
//! {"event_kind":"assign",...}
//! ```
//!
//! The schema line is intentionally trivial — enough to reject unknown
//! future formats without pulling in a richer framing format.

use crate::event::Event;

/// Current archive schema string. Stored verbatim in each archive's
/// header line so older readers can refuse newer blobs rather than
/// mis-parse them.
pub const ARCHIVE_SCHEMA_V1: &str = "kinora-commit-archive-v1";

/// Content-blob kind for archive kinos stored in `ContentStore`. Distinct
/// from user-content kinds (`markdown`, `styx`, …) so downstream tooling
/// can recognize and skip them in generic views.
pub const ARCHIVE_CONTENT_KIND: &str = "commit-archive";

#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    #[error("archive is empty: expected a schema header on the first line")]
    Empty,
    #[error("archive header parse error: {0}")]
    HeaderParse(String),
    #[error("unsupported archive schema: {0:?} (expected {expected:?})", expected = ARCHIVE_SCHEMA_V1)]
    UnsupportedSchema(String),
    #[error("event parse error on line {line}: {err}")]
    EventParse { line: usize, err: String },
    #[error("event serialize error: {0}")]
    EventSerialize(String),
}

/// Serialize a sequence of events into the v1 archive wire format.
///
/// Order is preserved byte-for-byte — the caller picks the commit order.
/// The output ends with a trailing LF so naive concatenation / text-mode
/// tooling produces a clean file.
pub fn serialize_archive(_events: &[Event]) -> Result<Vec<u8>, ArchiveError> {
    todo!("kinora-q6bo Phase B impl")
}

/// Parse a v1 archive blob back into `(header-schema, events)`.
///
/// The schema string is returned verbatim to let callers tell apart
/// future schema variants if they're added. Empty bodies (header only)
/// parse to `Vec::new()`. A trailing LF is tolerated but not required.
pub fn parse_archive(_bytes: &[u8]) -> Result<(String, Vec<Event>), ArchiveError> {
    todo!("kinora-q6bo Phase B impl")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EVENT_KIND_STORE;
    use crate::hash::Hash;
    use std::collections::BTreeMap;

    fn mk_event(body: &[u8], name: &str, ts: &str) -> Event {
        let h = Hash::of_content(body);
        Event::new_store(
            "markdown".into(),
            h.as_hex().into(),
            h.as_hex().into(),
            vec![],
            ts.into(),
            "yj".into(),
            "test".into(),
            BTreeMap::from([("name".to_owned(), name.to_owned())]),
        )
    }

    #[test]
    fn roundtrip_empty_events() {
        let bytes = serialize_archive(&[]).unwrap();
        let (schema, events) = parse_archive(&bytes).unwrap();
        assert_eq!(schema, ARCHIVE_SCHEMA_V1);
        assert!(events.is_empty());
    }

    #[test]
    fn roundtrip_single_event() {
        let e = mk_event(b"a", "alpha", "2026-04-19T10:00:00Z");
        let bytes = serialize_archive(std::slice::from_ref(&e)).unwrap();
        let (schema, events) = parse_archive(&bytes).unwrap();
        assert_eq!(schema, ARCHIVE_SCHEMA_V1);
        assert_eq!(events, vec![e]);
    }

    #[test]
    fn roundtrip_preserves_event_order() {
        let a = mk_event(b"a", "alpha", "2026-04-19T10:00:00Z");
        let b = mk_event(b"b", "bravo", "2026-04-19T10:00:01Z");
        let c = mk_event(b"c", "charlie", "2026-04-19T10:00:02Z");
        let bytes = serialize_archive(&[a.clone(), b.clone(), c.clone()]).unwrap();
        let (_schema, events) = parse_archive(&bytes).unwrap();
        assert_eq!(events, vec![a, b, c]);
    }

    #[test]
    fn first_line_is_schema_header() {
        let e = mk_event(b"a", "alpha", "2026-04-19T10:00:00Z");
        let bytes = serialize_archive(std::slice::from_ref(&e)).unwrap();
        let s = std::str::from_utf8(&bytes).unwrap();
        let first = s.lines().next().unwrap();
        assert_eq!(first, r#"{"@schema":"kinora-commit-archive-v1"}"#);
    }

    #[test]
    fn each_event_on_its_own_line() {
        let a = mk_event(b"a", "alpha", "2026-04-19T10:00:00Z");
        let b = mk_event(b"b", "bravo", "2026-04-19T10:00:01Z");
        let bytes = serialize_archive(&[a, b]).unwrap();
        let s = std::str::from_utf8(&bytes).unwrap();
        let non_empty: Vec<_> = s.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(non_empty.len(), 3, "got: {s:?}");
    }

    #[test]
    fn trailing_newline_is_tolerated_on_parse() {
        let e = mk_event(b"a", "alpha", "2026-04-19T10:00:00Z");
        let bytes = serialize_archive(std::slice::from_ref(&e)).unwrap();
        let last = bytes.last().copied().unwrap();
        assert_eq!(last, b'\n', "serializer should end with LF");
        let (_s, evs) = parse_archive(&bytes).unwrap();
        assert_eq!(evs.len(), 1);
    }

    #[test]
    fn missing_trailing_newline_also_parses() {
        let mut bytes = serialize_archive(&[mk_event(
            b"a",
            "alpha",
            "2026-04-19T10:00:00Z",
        )])
        .unwrap();
        if bytes.last() == Some(&b'\n') {
            bytes.pop();
        }
        let (_s, evs) = parse_archive(&bytes).unwrap();
        assert_eq!(evs.len(), 1);
    }

    #[test]
    fn empty_bytes_rejected() {
        let err = parse_archive(&[]).unwrap_err();
        assert!(matches!(err, ArchiveError::Empty), "got: {err:?}");
    }

    #[test]
    fn missing_header_rejected() {
        let e = mk_event(b"a", "alpha", "2026-04-19T10:00:00Z");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(e.to_json_line().unwrap().as_bytes());
        bytes.push(b'\n');
        let err = parse_archive(&bytes).unwrap_err();
        assert!(
            matches!(err, ArchiveError::HeaderParse(_)),
            "got: {err:?}"
        );
    }

    #[test]
    fn unknown_schema_rejected() {
        let bytes = b"{\"@schema\":\"kinora-commit-archive-v999\"}\n";
        let err = parse_archive(bytes).unwrap_err();
        match err {
            ArchiveError::UnsupportedSchema(s) => {
                assert_eq!(s, "kinora-commit-archive-v999");
            }
            other => panic!("expected UnsupportedSchema, got {other:?}"),
        }
    }

    #[test]
    fn malformed_event_line_rejected_with_line_number() {
        let header = format!("{{\"@schema\":\"{ARCHIVE_SCHEMA_V1}\"}}\n");
        let good = mk_event(b"a", "alpha", "2026-04-19T10:00:00Z")
            .to_json_line()
            .unwrap();
        let bad = "{not json}";
        let blob = format!("{header}{good}\n{bad}\n");
        let err = parse_archive(blob.as_bytes()).unwrap_err();
        match err {
            ArchiveError::EventParse { line, .. } => {
                assert_eq!(line, 3);
            }
            other => panic!("expected EventParse, got {other:?}"),
        }
    }

    #[test]
    fn non_utf8_rejected() {
        let mut bytes = serialize_archive(&[mk_event(
            b"a",
            "alpha",
            "2026-04-19T10:00:00Z",
        )])
        .unwrap();
        bytes[5] = 0xFF;
        let err = parse_archive(&bytes).unwrap_err();
        assert!(matches!(err, ArchiveError::HeaderParse(_)), "got: {err:?}");
    }

    #[test]
    fn all_events_parsed_carry_expected_event_kind() {
        let store_evt = mk_event(b"x", "xray", "2026-04-19T10:00:00Z");
        let mut assign_evt = store_evt.clone();
        assign_evt.event_kind = "assign".into();
        let bytes = serialize_archive(&[store_evt.clone(), assign_evt.clone()]).unwrap();
        let (_s, evs) = parse_archive(&bytes).unwrap();
        assert_eq!(evs.len(), 2);
        assert_eq!(evs[0].event_kind, EVENT_KIND_STORE);
        assert_eq!(evs[1].event_kind, "assign");
    }
}
