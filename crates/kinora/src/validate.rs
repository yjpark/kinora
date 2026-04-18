use std::str::FromStr;

use crate::event::Event;
use crate::hash::{Hash, HashParseError};
use crate::namespace::{self, NamespaceError};
use crate::store::ContentStore;

#[derive(Debug)]
pub enum ValidationError {
    Namespace(NamespaceError),
    InvalidHash { field: &'static str, value: String, err: HashParseError },
    BirthEventMustHaveNoParents,
    BirthEventIdMustEqualHash,
    VersionEventIdMustDiffer,
    ParentNotInStore { hash: Hash },
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidationError::Namespace(e) => write!(f, "{e}"),
            ValidationError::InvalidHash { field, value, err } => {
                write!(f, "invalid hash in `{field}`: {value} — {err}")
            }
            ValidationError::BirthEventMustHaveNoParents => {
                write!(f, "birth event must have empty parents[]")
            }
            ValidationError::BirthEventIdMustEqualHash => {
                write!(f, "birth event id must equal hash")
            }
            ValidationError::VersionEventIdMustDiffer => {
                write!(f, "version event id must differ from hash")
            }
            ValidationError::ParentNotInStore { hash } => {
                write!(f, "parent hash not present in store: {hash}")
            }
        }
    }
}

impl std::error::Error for ValidationError {}

impl From<NamespaceError> for ValidationError {
    fn from(e: NamespaceError) -> Self {
        ValidationError::Namespace(e)
    }
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
    parse_hash_field("hash", &event.hash)?;
    for p in &event.parents {
        parse_hash_field("parents[]", p)?;
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

/// Verify each parent hash exists in the content store.
pub fn validate_parents_exist(
    store: &ContentStore,
    event: &Event,
) -> Result<(), ValidationError> {
    for p in &event.parents {
        let h = parse_hash_field("parents[]", p)?;
        if !store.exists(&h) {
            return Err(ValidationError::ParentNotInStore { hash: h });
        }
    }
    Ok(())
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
        Event {
            kind: kind.into(),
            id: h.as_hex().into(),
            hash: h.as_hex().into(),
            parents: vec![],
            ts: "2026-04-18T09:00:00Z".into(),
            author: "yj".into(),
            provenance: "test".into(),
            metadata: BTreeMap::new(),
        }
    }

    fn version_from(birth: &Event) -> Event {
        let new_hash = Hash::of_content(b"content-v2");
        Event {
            kind: birth.kind.clone(),
            id: birth.id.clone(),
            hash: new_hash.as_hex().into(),
            parents: vec![birth.hash.clone()],
            ts: "2026-04-18T09:01:00Z".into(),
            author: birth.author.clone(),
            provenance: birth.provenance.clone(),
            metadata: BTreeMap::new(),
        }
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
        e.parents.push(e.hash.clone());
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

        let stored_hash = store.write(b"prior").unwrap();

        let mut e = version_from(&birth("markdown"));
        e.parents = vec![stored_hash.as_hex().into()];
        assert!(validate_parents_exist(&store, &e).is_ok());

        e.parents.push(Hash::of_content(b"missing").as_hex().into());
        let err = validate_parents_exist(&store, &e).unwrap_err();
        assert!(matches!(err, ValidationError::ParentNotInStore { .. }));
    }
}
