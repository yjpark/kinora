use std::collections::HashSet;
use std::str::FromStr;

use crate::event::Event;
use crate::hash::{Hash, HashParseError};
use crate::namespace::{self, NamespaceError};
use crate::store::{ContentStore, StoreError};

#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error(transparent)]
    Namespace(#[from] NamespaceError),
    #[error("invalid hash in `{field}`: {value} — {err}")]
    InvalidHash {
        field: &'static str,
        value: String,
        #[source]
        err: HashParseError,
    },
    #[error("birth event must have empty parents[]")]
    BirthEventMustHaveNoParents,
    #[error("birth event id must equal hash")]
    BirthEventIdMustEqualHash,
    #[error("version event id must differ from hash")]
    VersionEventIdMustDiffer,
    #[error("parent hash not present in store: {hash}")]
    ParentNotInStore { hash: Hash },
    #[error("parent content corrupted for {hash}: {err}")]
    ParentCorrupted {
        hash: Hash,
        #[source]
        err: StoreError,
    },
    #[error("event cannot list its own hash as a parent: {hash}")]
    SelfParenting { hash: Hash },
    #[error("duplicate parent hash: {hash}")]
    DuplicateParent { hash: Hash },
    #[error("event hash not present in store: {hash}")]
    EventHashNotInStore { hash: Hash },
    #[error("event content corrupted for {hash}: {err}")]
    EventHashCorrupted {
        hash: Hash,
        #[source]
        err: StoreError,
    },
}

/// Validate the shape of an event: kind namespacing, metadata key
/// namespacing, hash field parseability, and birth/version consistency.
///
/// An event is treated as a birth event iff `parents` is empty and
/// `id == hash`. Anything else is a version event.
pub fn validate_event_shape(event: &Event) -> Result<(), ValidationError> {
    namespace::validate_kind(&event.kind)?;
    for key in event.metadata.keys() {
        namespace::validate_metadata_key(key)?;
    }
    parse_hash_field("id", &event.id)?;
    let self_hash = parse_hash_field("hash", &event.hash)?;
    let mut seen: HashSet<Hash> = HashSet::new();
    for p in &event.parents {
        let ph = parse_hash_field("parents[]", p)?;
        if ph == self_hash {
            return Err(ValidationError::SelfParenting { hash: ph });
        }
        if !seen.insert(ph.clone()) {
            return Err(ValidationError::DuplicateParent { hash: ph });
        }
    }
    if event.parents.is_empty() {
        if event.id != event.hash {
            return Err(ValidationError::BirthEventIdMustEqualHash);
        }
    } else if event.id == event.hash {
        return Err(ValidationError::VersionEventIdMustDiffer);
    }
    Ok(())
}

/// Verify each parent hash resolves to intact content in the store.
///
/// Uses `store.read()` so the stored bytes are re-hashed — catches
/// corruption or tampering, not just file presence.
pub fn validate_parents_exist(
    store: &ContentStore,
    event: &Event,
) -> Result<(), ValidationError> {
    for p in &event.parents {
        let h = parse_hash_field("parents[]", p)?;
        match store.read(&h) {
            Ok(_) => {}
            Err(StoreError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(ValidationError::ParentNotInStore { hash: h });
            }
            Err(err) => return Err(ValidationError::ParentCorrupted { hash: h, err }),
        }
    }
    Ok(())
}

/// Verify that `event.hash` resolves to intact content in the store.
///
/// Complements `validate_parents_exist` by binding the event to its own
/// stored bytes — without this, an event envelope could reference a hash
/// whose content was never written.
pub fn validate_event_hash_in_store(
    store: &ContentStore,
    event: &Event,
) -> Result<(), ValidationError> {
    let h = parse_hash_field("hash", &event.hash)?;
    match store.read(&h) {
        Ok(_) => Ok(()),
        Err(StoreError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(ValidationError::EventHashNotInStore { hash: h })
        }
        Err(err) => Err(ValidationError::EventHashCorrupted { hash: h, err }),
    }
}

fn parse_hash_field(field: &'static str, value: &str) -> Result<Hash, ValidationError> {
    Hash::from_str(value).map_err(|err| ValidationError::InvalidHash {
        field,
        value: value.to_owned(),
        err,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use tempfile::TempDir;

    fn birth(kind: &str) -> Event {
        let h = Hash::of_content(b"content");
        Event::new_store(
            kind.into(),
            h.as_hex().into(),
            h.as_hex().into(),
            vec![],
            "2026-04-18T09:00:00Z".into(),
            "yj".into(),
            "test".into(),
            BTreeMap::new(),
        )
    }

    fn version_from(birth: &Event) -> Event {
        let new_hash = Hash::of_content(b"content-v2");
        Event::new_store(
            birth.kind.clone(),
            birth.id.clone(),
            new_hash.as_hex().into(),
            vec![birth.hash.clone()],
            "2026-04-18T09:01:00Z".into(),
            birth.author.clone(),
            birth.provenance.clone(),
            BTreeMap::new(),
        )
    }

    #[test]
    fn valid_birth_event_passes() {
        assert!(validate_event_shape(&birth("markdown")).is_ok());
    }

    #[test]
    fn valid_version_event_passes() {
        let b = birth("markdown");
        let v = version_from(&b);
        assert!(validate_event_shape(&v).is_ok());
    }

    #[test]
    fn unknown_bare_kind_rejected() {
        let mut e = birth("random");
        e.kind = "random".into();
        let err = validate_event_shape(&e).unwrap_err();
        assert!(matches!(err, ValidationError::Namespace(_)));
    }

    #[test]
    fn namespaced_kind_accepted() {
        let e = birth("kudo::diagram");
        assert!(validate_event_shape(&e).is_ok());
    }

    #[test]
    fn unknown_bare_metadata_key_rejected() {
        let mut e = birth("markdown");
        e.metadata.insert("weird".into(), "x".into());
        let err = validate_event_shape(&e).unwrap_err();
        assert!(matches!(err, ValidationError::Namespace(_)));
    }

    #[test]
    fn birth_with_parents_rejected() {
        let mut e = birth("markdown");
        e.parents.push(Hash::of_content(b"prior").as_hex().into());
        let err = validate_event_shape(&e).unwrap_err();
        assert!(matches!(err, ValidationError::VersionEventIdMustDiffer));
    }

    #[test]
    fn birth_with_id_ne_hash_rejected() {
        let mut e = birth("markdown");
        e.id = Hash::of_content(b"other").as_hex().into();
        let err = validate_event_shape(&e).unwrap_err();
        assert!(matches!(err, ValidationError::BirthEventIdMustEqualHash));
    }

    #[test]
    fn version_with_id_eq_hash_rejected() {
        let mut e = version_from(&birth("markdown"));
        e.parents = vec![Hash::of_content(b"other-parent").as_hex().into()];
        e.hash = e.id.clone();
        let err = validate_event_shape(&e).unwrap_err();
        assert!(matches!(err, ValidationError::VersionEventIdMustDiffer));
    }

    #[test]
    fn invalid_hash_rejected() {
        let mut e = birth("markdown");
        e.id = "not-a-hex-hash".into();
        let err = validate_event_shape(&e).unwrap_err();
        assert!(matches!(err, ValidationError::InvalidHash { .. }));
    }

    #[test]
    fn parent_existence_checked() {
        let tmp = TempDir::new().unwrap();
        let store = ContentStore::new(tmp.path());
        store.ensure_layout().unwrap();

        let stored_hash = store.write("markdown", b"prior").unwrap();

        let mut e = version_from(&birth("markdown"));
        e.parents = vec![stored_hash.as_hex().into()];
        assert!(validate_parents_exist(&store, &e).is_ok());

        e.parents.push(Hash::of_content(b"missing").as_hex().into());
        let err = validate_parents_exist(&store, &e).unwrap_err();
        assert!(matches!(err, ValidationError::ParentNotInStore { .. }));
    }

    #[test]
    fn self_parenting_rejected() {
        let mut e = version_from(&birth("markdown"));
        e.parents = vec![e.hash.clone()];
        let err = validate_event_shape(&e).unwrap_err();
        assert!(matches!(err, ValidationError::SelfParenting { .. }));
    }

    #[test]
    fn duplicate_parents_rejected() {
        let mut e = version_from(&birth("markdown"));
        let p = Hash::of_content(b"prior").as_hex().into();
        e.parents = vec![p, Hash::of_content(b"prior").as_hex().into()];
        let err = validate_event_shape(&e).unwrap_err();
        assert!(matches!(err, ValidationError::DuplicateParent { .. }));
    }

    #[test]
    fn parent_corruption_detected() {
        use crate::paths::find_blob_path;
        let tmp = TempDir::new().unwrap();
        let store = ContentStore::new(tmp.path());
        store.ensure_layout().unwrap();

        let stored_hash = store.write("markdown", b"prior").unwrap();
        let path = find_blob_path(store.root(), &stored_hash).unwrap();
        std::fs::write(&path, b"tampered").unwrap();

        let mut e = version_from(&birth("markdown"));
        e.parents = vec![stored_hash.as_hex().into()];
        let err = validate_parents_exist(&store, &e).unwrap_err();
        assert!(matches!(err, ValidationError::ParentCorrupted { .. }));
    }

    #[test]
    fn event_hash_in_store_checks_presence_and_integrity() {
        use crate::paths::find_blob_path;
        let tmp = TempDir::new().unwrap();
        let store = ContentStore::new(tmp.path());
        store.ensure_layout().unwrap();

        let content = b"event-content";
        let h = store.write("markdown", content).unwrap();
        let mut e = birth("markdown");
        e.id = h.as_hex().into();
        e.hash = h.as_hex().into();
        assert!(validate_event_hash_in_store(&store, &e).is_ok());

        // missing blob
        let mut missing = e.clone();
        let other = Hash::of_content(b"not-written");
        missing.id = other.as_hex().into();
        missing.hash = other.as_hex().into();
        let err = validate_event_hash_in_store(&store, &missing).unwrap_err();
        assert!(matches!(err, ValidationError::EventHashNotInStore { .. }));

        // corrupted blob
        let path = find_blob_path(store.root(), &h).unwrap();
        std::fs::write(&path, b"tampered").unwrap();
        let err = validate_event_hash_in_store(&store, &e).unwrap_err();
        assert!(matches!(err, ValidationError::EventHashCorrupted { .. }));
    }
}
