use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::event::{Event, EventError};
pub use crate::event::EVENT_KIND_ASSIGN;
use crate::hash::Hash;
use crate::ledger::{Ledger, LedgerError};

/// `Event::kind` value for assign events. Namespaced under `kin::` so
/// assign events pass the same kind-validation as store events while
/// remaining distinct from any content kind. The true track discriminator
/// is `event_kind = "assign"` (see `EVENT_KIND_ASSIGN`).
pub const ASSIGN_KIND_TAG: &str = "kin::assign";

/// Metadata key under which an assign event carries its target root name.
/// Namespaced under `kin::` so it's unambiguously a protocol field and
/// cannot collide with user or extension metadata.
pub const META_TARGET_ROOT: &str = "kin::target_root";

/// A kino→root assignment. Names a kino by id, a target root by name, and
/// (optionally) a list of prior assign-event hashes this one supersedes.
///
/// Assign events carry no content blob — they only reshape the kino→root
/// graph. Phase 3.5 (kinora-7mou) teaches commit to consume them; until
/// then they live in the staged ledger and are transparently ignored by
/// content-store consumers (resolver, render, commit today) via
/// `Event::is_store_event()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignEvent {
    pub kino_id: String,
    pub target_root: String,
    pub supersedes: Vec<String>,
    pub author: String,
    pub ts: String,
    pub provenance: String,
}

#[derive(Debug, thiserror::Error)]
pub enum AssignError {
    #[error(transparent)]
    Ledger(#[from] LedgerError),
    #[error(transparent)]
    Event(#[from] EventError),
    #[error(".kinora/ not found at {}; run `kinora init` first", .path.display())]
    KinoraMissing { path: PathBuf },
    #[error("expected event_kind=\"{}\", got {event_kind:?}", EVENT_KIND_ASSIGN)]
    NotAssignEvent { event_kind: String },
    #[error("assign event kind must be `{}`, got {kind:?}", ASSIGN_KIND_TAG)]
    WrongKind { kind: String },
    #[error("assign event id must equal hash (kino_id placeholder); id={id}, hash={hash}")]
    IdHashMismatch { id: String, hash: String },
    #[error("assign event `{field}` is not a valid hex hash: {value}")]
    InvalidHash { field: &'static str, value: String },
    #[error("assign event missing metadata key `{}`", META_TARGET_ROOT)]
    MissingTargetRoot,
    #[error("assign target_root must be non-empty")]
    EmptyTargetRoot,
    #[error("assign kino_id must be non-empty")]
    EmptyKinoId,
}

impl AssignEvent {
    /// Convert to the on-wire `Event` form. Assign events reuse the `Event`
    /// struct so they flow through the same staged-ledger write/read path as
    /// store events. Field mapping:
    ///
    /// | Event field    | AssignEvent source                    |
    /// |----------------|---------------------------------------|
    /// | `event_kind`   | `"assign"`                            |
    /// | `kind`         | `"kin::assign"`                       |
    /// | `id`           | `kino_id`                             |
    /// | `hash`         | `kino_id` (placeholder — no blob)     |
    /// | `parents`      | `supersedes`                          |
    /// | `metadata`     | `{ "kin::target_root": target_root }` |
    ///
    /// `hash` is set to `kino_id` so the structural id==hash invariant a
    /// future validator might check still holds; assign events carry no
    /// content blob, so the field is effectively a placeholder.
    pub fn to_event(&self) -> Event {
        let mut metadata = BTreeMap::new();
        metadata.insert(META_TARGET_ROOT.to_owned(), self.target_root.clone());
        Event {
            event_kind: EVENT_KIND_ASSIGN.to_owned(),
            kind: ASSIGN_KIND_TAG.to_owned(),
            id: self.kino_id.clone(),
            hash: self.kino_id.clone(),
            parents: self.supersedes.clone(),
            ts: self.ts.clone(),
            author: self.author.clone(),
            provenance: self.provenance.clone(),
            metadata,
        }
    }

    /// Parse a wire `Event` back into an `AssignEvent`. Enforces the
    /// protocol invariants produced by `to_event`:
    ///
    /// - `event_kind == "assign"`
    /// - `kind == "kin::assign"`
    /// - `id == hash` (the shared kino_id placeholder)
    /// - `id` parses as a hex hash
    /// - metadata contains `kin::target_root`
    ///
    /// A nonconformant producer therefore surfaces as an explicit error
    /// rather than silently round-tripping into a malformed `AssignEvent`.
    /// Additional metadata entries beyond `kin::target_root` are dropped.
    pub fn from_event(e: &Event) -> Result<Self, AssignError> {
        if e.event_kind != EVENT_KIND_ASSIGN {
            return Err(AssignError::NotAssignEvent {
                event_kind: e.event_kind.clone(),
            });
        }
        if e.kind != ASSIGN_KIND_TAG {
            return Err(AssignError::WrongKind { kind: e.kind.clone() });
        }
        if e.id != e.hash {
            return Err(AssignError::IdHashMismatch {
                id: e.id.clone(),
                hash: e.hash.clone(),
            });
        }
        Hash::from_str(&e.id).map_err(|_| AssignError::InvalidHash {
            field: "kino_id",
            value: e.id.clone(),
        })?;
        let target_root = e
            .metadata
            .get(META_TARGET_ROOT)
            .ok_or(AssignError::MissingTargetRoot)?
            .clone();
        Ok(AssignEvent {
            kino_id: e.id.clone(),
            target_root,
            supersedes: e.parents.clone(),
            author: e.author.clone(),
            ts: e.ts.clone(),
            provenance: e.provenance.clone(),
        })
    }

    /// BLAKE3 of this assign event's canonical JSON line (via `to_event()`).
    /// Same content-addressing discipline as `Event::event_hash()`.
    pub fn event_hash(&self) -> Result<Hash, AssignError> {
        Ok(self.to_event().event_hash()?)
    }
}

/// Write `a` to `.kinora/staged/<ab>/<event-hash>.jsonl`. Returns
/// `(event_hash, was_new)` — `was_new` is true iff the target file did
/// not exist before this call. Idempotent: re-writing the same logical
/// assign event is a no-op.
///
/// Errors before any filesystem work if `kinora_root` does not already
/// exist (parity with `store_kino`'s UX), if `kino_id` or `target_root`
/// are empty, or if `kino_id` / any `supersedes` entry is not a valid
/// hex hash.
pub fn write_assign(
    kinora_root: &Path,
    a: &AssignEvent,
) -> Result<(Hash, bool), AssignError> {
    if !kinora_root.exists() {
        return Err(AssignError::KinoraMissing { path: kinora_root.to_path_buf() });
    }
    if a.kino_id.is_empty() {
        return Err(AssignError::EmptyKinoId);
    }
    if a.target_root.is_empty() {
        return Err(AssignError::EmptyTargetRoot);
    }
    Hash::from_str(&a.kino_id).map_err(|_| AssignError::InvalidHash {
        field: "kino_id",
        value: a.kino_id.clone(),
    })?;
    for s in &a.supersedes {
        Hash::from_str(s).map_err(|_| AssignError::InvalidHash {
            field: "supersedes[]",
            value: s.clone(),
        })?;
    }
    let ledger = Ledger::new(kinora_root);
    ledger.ensure_layout()?;
    let event = a.to_event();
    Ok(ledger.write_event(&event)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::init::init;
    use crate::paths::kinora_root;
    use tempfile::TempDir;

    fn sample() -> AssignEvent {
        let kino = Hash::of_content(b"my-kino").as_hex().to_owned();
        AssignEvent {
            kino_id: kino,
            target_root: "main".into(),
            supersedes: vec![],
            author: "yj".into(),
            ts: "2026-04-19T10:00:00Z".into(),
            provenance: "test".into(),
        }
    }

    fn setup() -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        init(tmp.path(), "https://example.com/x.git").unwrap();
        let root = kinora_root(tmp.path());
        (tmp, root)
    }

    #[test]
    fn to_event_sets_event_kind_to_assign() {
        let e = sample().to_event();
        assert_eq!(e.event_kind, EVENT_KIND_ASSIGN);
        assert!(!e.is_store_event(), "assign event must not be a store event");
    }

    #[test]
    fn to_event_uses_namespaced_assign_kind_tag() {
        let e = sample().to_event();
        assert_eq!(e.kind, ASSIGN_KIND_TAG);
        assert_eq!(e.kind, "kin::assign");
    }

    #[test]
    fn to_event_puts_target_root_in_metadata_under_namespaced_key() {
        let a = sample();
        let e = a.to_event();
        assert_eq!(e.metadata.get(META_TARGET_ROOT), Some(&a.target_root));
    }

    #[test]
    fn to_event_uses_kino_id_as_id_and_hash() {
        let a = sample();
        let e = a.to_event();
        assert_eq!(e.id, a.kino_id);
        assert_eq!(e.hash, a.kino_id);
    }

    #[test]
    fn to_event_copies_supersedes_into_parents() {
        let mut a = sample();
        a.supersedes = vec![
            Hash::of_content(b"prior-a").as_hex().into(),
            Hash::of_content(b"prior-b").as_hex().into(),
        ];
        let e = a.to_event();
        assert_eq!(e.parents, a.supersedes);
    }

    #[test]
    fn from_event_inverts_to_event() {
        let a = sample();
        let e = a.to_event();
        let back = AssignEvent::from_event(&e).unwrap();
        assert_eq!(back, a);
    }

    #[test]
    fn from_event_with_supersedes_roundtrips() {
        let mut a = sample();
        a.supersedes = vec![
            Hash::of_content(b"prior-a").as_hex().into(),
            Hash::of_content(b"prior-b").as_hex().into(),
        ];
        let e = a.to_event();
        let back = AssignEvent::from_event(&e).unwrap();
        assert_eq!(back, a);
    }

    #[test]
    fn from_event_rejects_store_event() {
        let e = Event::new_store(
            "markdown".into(),
            Hash::of_content(b"x").as_hex().into(),
            Hash::of_content(b"x").as_hex().into(),
            vec![],
            "2026-04-19T10:00:00Z".into(),
            "yj".into(),
            "test".into(),
            BTreeMap::new(),
        );
        let err = AssignEvent::from_event(&e).unwrap_err();
        match err {
            AssignError::NotAssignEvent { event_kind } => {
                assert_eq!(event_kind, "store");
            }
            other => panic!("expected NotAssignEvent, got {other:?}"),
        }
    }

    #[test]
    fn from_event_missing_target_root_metadata_errors() {
        let mut e = sample().to_event();
        e.metadata.clear();
        let err = AssignEvent::from_event(&e).unwrap_err();
        assert!(matches!(err, AssignError::MissingTargetRoot), "got {err:?}");
    }

    #[test]
    fn event_hash_matches_underlying_event_hash() {
        let a = sample();
        let via_assign = a.event_hash().unwrap();
        let via_event = a.to_event().event_hash().unwrap();
        assert_eq!(via_assign, via_event);
    }

    #[test]
    fn event_hash_changes_with_target_root() {
        let a = sample();
        let mut b = sample();
        b.target_root = "drafts".into();
        assert_ne!(a.event_hash().unwrap(), b.event_hash().unwrap());
    }

    #[test]
    fn event_hash_changes_with_supersedes() {
        let a = sample();
        let mut b = sample();
        b.supersedes = vec![Hash::of_content(b"prior").as_hex().into()];
        assert_ne!(a.event_hash().unwrap(), b.event_hash().unwrap());
    }

    #[test]
    fn write_assign_creates_staged_event_file() {
        let (_tmp, root) = setup();
        let a = sample();
        let (h, was_new) = write_assign(&root, &a).unwrap();
        assert!(was_new);
        let path = crate::paths::staged_event_path(&root, &h);
        assert!(path.is_file(), "staged event file missing: {}", path.display());
    }

    #[test]
    fn write_assign_is_idempotent() {
        let (_tmp, root) = setup();
        let a = sample();
        let (h1, new1) = write_assign(&root, &a).unwrap();
        let (h2, new2) = write_assign(&root, &a).unwrap();
        assert_eq!(h1, h2);
        assert!(new1);
        assert!(!new2, "second write of identical assign must not be new");
    }

    #[test]
    fn write_assign_rejects_empty_kino_id() {
        let (_tmp, root) = setup();
        let mut a = sample();
        a.kino_id = String::new();
        let err = write_assign(&root, &a).unwrap_err();
        assert!(matches!(err, AssignError::EmptyKinoId), "got {err:?}");
    }

    #[test]
    fn write_assign_rejects_empty_target_root() {
        let (_tmp, root) = setup();
        let mut a = sample();
        a.target_root = String::new();
        let err = write_assign(&root, &a).unwrap_err();
        assert!(matches!(err, AssignError::EmptyTargetRoot), "got {err:?}");
    }

    #[test]
    fn write_assign_rejects_invalid_kino_id_hash() {
        let (_tmp, root) = setup();
        let mut a = sample();
        a.kino_id = "not-a-hex-hash".into();
        let err = write_assign(&root, &a).unwrap_err();
        match err {
            AssignError::InvalidHash { field, value } => {
                assert_eq!(field, "kino_id");
                assert_eq!(value, "not-a-hex-hash");
            }
            other => panic!("expected InvalidHash, got {other:?}"),
        }
    }

    #[test]
    fn write_assign_rejects_invalid_supersedes_hash() {
        let (_tmp, root) = setup();
        let mut a = sample();
        a.supersedes = vec!["garbage".into()];
        let err = write_assign(&root, &a).unwrap_err();
        match err {
            AssignError::InvalidHash { field, value } => {
                assert_eq!(field, "supersedes[]");
                assert_eq!(value, "garbage");
            }
            other => panic!("expected InvalidHash, got {other:?}"),
        }
    }

    #[test]
    fn write_assign_errors_when_kinora_missing() {
        let tmp = TempDir::new().unwrap();
        // No init — no .kinora/ directory.
        let missing = tmp.path().join(".kinora");
        let err = write_assign(&missing, &sample()).unwrap_err();
        assert!(matches!(err, AssignError::KinoraMissing { .. }), "got {err:?}");
    }

    #[test]
    fn write_assign_event_is_readable_via_ledger_read_all_events() {
        let (_tmp, root) = setup();
        let a = sample();
        write_assign(&root, &a).unwrap();

        let events = Ledger::new(&root).read_all_events().unwrap();
        assert_eq!(events.len(), 1);
        let e = &events[0];
        assert_eq!(e.event_kind, EVENT_KIND_ASSIGN);
        assert!(!e.is_store_event());

        let back = AssignEvent::from_event(e).unwrap();
        assert_eq!(back, a);
    }

    #[test]
    fn write_assign_different_kinos_produce_distinct_files() {
        let (_tmp, root) = setup();
        let a = sample();
        let mut b = sample();
        b.kino_id = Hash::of_content(b"other-kino").as_hex().to_owned();

        let (ha, _) = write_assign(&root, &a).unwrap();
        let (hb, _) = write_assign(&root, &b).unwrap();
        assert_ne!(ha, hb);

        let events = Ledger::new(&root).read_all_events().unwrap();
        assert_eq!(events.len(), 2);
    }

    // --- from_event strictness (review fixes) ---

    #[test]
    fn from_event_rejects_wrong_kind() {
        let mut e = sample().to_event();
        e.kind = "markdown".into();
        let err = AssignEvent::from_event(&e).unwrap_err();
        match err {
            AssignError::WrongKind { kind } => assert_eq!(kind, "markdown"),
            other => panic!("expected WrongKind, got {other:?}"),
        }
    }

    #[test]
    fn from_event_rejects_id_hash_mismatch() {
        let mut e = sample().to_event();
        e.hash = Hash::of_content(b"other").as_hex().into();
        let err = AssignEvent::from_event(&e).unwrap_err();
        assert!(matches!(err, AssignError::IdHashMismatch { .. }), "got {err:?}");
    }

    #[test]
    fn from_event_rejects_invalid_kino_id_hash() {
        let mut e = sample().to_event();
        e.id = "not-a-hash".into();
        e.hash = "not-a-hash".into();
        let err = AssignEvent::from_event(&e).unwrap_err();
        match err {
            AssignError::InvalidHash { field, value } => {
                assert_eq!(field, "kino_id");
                assert_eq!(value, "not-a-hash");
            }
            other => panic!("expected InvalidHash, got {other:?}"),
        }
    }

    #[test]
    fn from_event_drops_extra_metadata_keys() {
        // Only `kin::target_root` is promoted back into the domain struct;
        // any other metadata entries are dropped. Pins the round-trip
        // "forget extras" semantics.
        let mut e = sample().to_event();
        e.metadata.insert("kin::extra".into(), "ignored".into());
        let back = AssignEvent::from_event(&e).unwrap();
        assert_eq!(back, sample());
    }
}
