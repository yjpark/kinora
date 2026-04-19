use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use crate::event::{Event, EventError};
use crate::ledger::{Ledger, LedgerError};
use crate::store::{ContentStore, StoreError};
use crate::validate::{self, ValidationError};

#[derive(Debug, thiserror::Error)]
pub enum StoreKinoError {
    #[error("store-kino io error: {0}")]
    Io(#[from] io::Error),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Ledger(#[from] LedgerError),
    #[error(transparent)]
    Event(#[from] EventError),
    #[error(transparent)]
    Validation(#[from] ValidationError),
    #[error(".kinora/ not found at {}; run `kinora init` first", .path.display())]
    KinoraMissing { path: PathBuf },
    #[error("--parents requires --id: cannot infer identity from parents without walking the ledger DAG")]
    ParentsWithoutId,
}

/// Inputs for storing a new kino version (birth or version event).
///
/// A birth event is recorded when `id` is `None` *and* `parents` is empty;
/// id and hash are then both set to `BLAKE3(content)`. Otherwise it's a
/// version event and `id` must be provided — the caller is responsible for
/// linking to the correct identity.
#[derive(Debug, Clone)]
pub struct StoreKinoParams {
    pub kind: String,
    pub content: Vec<u8>,
    pub author: String,
    pub provenance: String,
    pub ts: String,
    pub metadata: BTreeMap<String, String>,
    pub id: Option<String>,
    pub parents: Vec<String>,
}

#[derive(Debug)]
pub struct StoredKino {
    pub event: Event,
    /// Shorthash of the event hash (first 8 hex chars). Under the staged-ledger
    /// layout (kinora-xi21) every event lives in its own file keyed by the
    /// event hash, so this is really the event shorthash. The field name is
    /// kept through one release for programmatic back-compat (kinora-6395);
    /// the CLI no longer surfaces the "lineage" wording.
    pub lineage: String,
    /// True iff this event's staged-ledger file did not already exist — i.e.
    /// this call introduced a new event. Idempotent re-stores return false.
    /// Retained under the `was_new_lineage` name for back-compat with
    /// programmatic callers; semantically now "was a new event".
    pub was_new_lineage: bool,
}

/// Write `params.content` to the content store (deduped) and record the
/// corresponding event in the staged ledger (`.kinora/staged/<ab>/<event-hash>.jsonl`).
/// Idempotent at the event-hash level: re-storing the same logical event
/// is a no-op on disk and returns `was_new_lineage=false`.
#[fastrace::trace]
pub fn store_kino(
    kinora_root: &Path,
    params: StoreKinoParams,
) -> Result<StoredKino, StoreKinoError> {
    if !kinora_root.exists() {
        return Err(StoreKinoError::KinoraMissing { path: kinora_root.to_path_buf() });
    }
    let store = ContentStore::new(kinora_root);
    store.ensure_layout()?;
    let ledger = Ledger::new(kinora_root);
    ledger.ensure_layout()?;

    let hash = store.write(&params.kind, &params.content)?;
    let hash_hex: String = hash.as_hex().into();

    let id = match params.id {
        Some(explicit) => explicit,
        None => {
            if !params.parents.is_empty() {
                // Version event without explicit id — disallow: we can't infer
                // identity from parents without walking the ledger DAG, and
                // mistakenly birthing-from-parents would silently detach.
                return Err(StoreKinoError::ParentsWithoutId);
            }
            hash_hex.clone()
        }
    };

    let event = Event {
        event_kind: crate::event::EVENT_KIND_STORE.to_owned(),
        kind: params.kind,
        id,
        hash: hash_hex,
        parents: params.parents,
        ts: params.ts,
        author: params.author,
        provenance: params.provenance,
        metadata: params.metadata,
    };

    validate::validate_event_shape(&event)?;
    validate::validate_parents_exist(&store, &event)?;

    let (event_hash, was_new_lineage) = ledger.write_event(&event)?;
    let lineage = event_hash.shorthash().to_owned();

    Ok(StoredKino { event, lineage, was_new_lineage })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::Hash;
    use crate::init::init;
    use crate::paths::{find_blob_path, kinora_root};
    use std::str::FromStr;
    use tempfile::TempDir;

    fn setup() -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        init(tmp.path(), "https://example.com/x.git").unwrap();
        let root = kinora_root(tmp.path());
        (tmp, root)
    }

    fn params(kind: &str, content: &[u8]) -> StoreKinoParams {
        StoreKinoParams {
            kind: kind.into(),
            content: content.to_vec(),
            author: "yj".into(),
            provenance: "test".into(),
            ts: "2026-04-18T10:00:00Z".into(),
            metadata: BTreeMap::from([("name".into(), "doc".into())]),
            id: None,
            parents: vec![],
        }
    }

    #[test]
    fn birth_event_creates_blob_and_mints_lineage() {
        let (_tmp, root) = setup();
        let stored = store_kino(&root, params("markdown", b"hello world")).unwrap();
        assert!(stored.was_new_lineage);
        assert_eq!(stored.event.id, stored.event.hash);
        assert!(stored.event.parents.is_empty());
        let hash = Hash::from_str(&stored.event.hash).unwrap();
        assert!(find_blob_path(&root, &hash).is_some());
    }

    #[test]
    fn each_store_creates_a_distinct_staged_event_file() {
        // Under the staged-ledger layout each event lives in its own file. Two
        // stores of different content produce two distinct event files, and
        // both events should be readable via `read_all_events`.
        let (_tmp, root) = setup();
        let first = store_kino(&root, params("markdown", b"a")).unwrap();
        let second = store_kino(&root, params("markdown", b"b")).unwrap();
        assert!(first.was_new_lineage);
        assert!(second.was_new_lineage);
        assert_ne!(first.lineage, second.lineage);
        let events = Ledger::new(&root).read_all_events().unwrap();
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn re_storing_the_same_event_is_idempotent_on_disk() {
        let (_tmp, root) = setup();
        let first = store_kino(&root, params("markdown", b"dup")).unwrap();
        let second = store_kino(&root, params("markdown", b"dup")).unwrap();
        assert!(first.was_new_lineage);
        assert!(!second.was_new_lineage, "same event twice should not be new");
        assert_eq!(first.event, second.event);
        assert_eq!(first.lineage, second.lineage);
        let events = Ledger::new(&root).read_all_events().unwrap();
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn version_event_requires_parents_to_exist() {
        let (_tmp, root) = setup();
        let birth = store_kino(&root, params("markdown", b"v1")).unwrap();

        let mut p = params("markdown", b"v2");
        p.id = Some(birth.event.id.clone());
        p.parents = vec![birth.event.hash.clone()];
        let version = store_kino(&root, p).unwrap();
        assert_eq!(version.event.id, birth.event.id);
        assert_ne!(version.event.hash, birth.event.hash);
        assert_eq!(version.event.parents, vec![birth.event.hash]);
    }

    #[test]
    fn version_event_rejects_missing_parent() {
        let (_tmp, root) = setup();
        let birth = store_kino(&root, params("markdown", b"v1")).unwrap();

        let mut p = params("markdown", b"v2");
        p.id = Some(birth.event.id.clone());
        p.parents = vec![Hash::of_content(b"never-stored").as_hex().into()];
        let err = store_kino(&root, p).unwrap_err();
        assert!(matches!(
            err,
            StoreKinoError::Validation(ValidationError::ParentNotInStore { .. })
        ));
    }

    #[test]
    fn parents_without_id_rejected() {
        let (_tmp, root) = setup();
        let birth = store_kino(&root, params("markdown", b"v1")).unwrap();

        let mut p = params("markdown", b"v2");
        p.parents = vec![birth.event.hash.clone()];
        let err = store_kino(&root, p).unwrap_err();
        assert!(matches!(err, StoreKinoError::ParentsWithoutId));
    }

    #[test]
    fn unknown_bare_kind_rejected() {
        let (_tmp, root) = setup();
        let err = store_kino(&root, params("random", b"x")).unwrap_err();
        assert!(matches!(
            err,
            StoreKinoError::Validation(ValidationError::Namespace(_))
        ));
    }

    #[test]
    fn unknown_bare_metadata_key_rejected() {
        let (_tmp, root) = setup();
        let mut p = params("markdown", b"x");
        p.metadata.insert("weird".into(), "v".into());
        let err = store_kino(&root, p).unwrap_err();
        assert!(matches!(
            err,
            StoreKinoError::Validation(ValidationError::Namespace(_))
        ));
    }

    #[test]
    fn dedupe_skips_blob_rewrite_but_still_records_distinct_events() {
        let (_tmp, root) = setup();
        let first = store_kino(&root, params("markdown", b"same")).unwrap();
        let hash = Hash::from_str(&first.event.hash).unwrap();
        let blob_path = find_blob_path(&root, &hash).unwrap();
        let mtime_before = std::fs::metadata(&blob_path).unwrap().modified().unwrap();

        // Same content, different `ts` → different logical event → different
        // event hash → separate file. The content blob is deduped, but each
        // event lives in its own staged file.
        let mut p = params("markdown", b"same");
        p.ts = "2026-04-18T10:00:01Z".into();
        let _second = store_kino(&root, p).unwrap();

        let mtime_after = std::fs::metadata(&blob_path).unwrap().modified().unwrap();
        assert_eq!(mtime_before, mtime_after, "blob rewritten");
        let events = Ledger::new(&root).read_all_events().unwrap();
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn kinora_missing_errors_clearly() {
        let tmp = TempDir::new().unwrap();
        // No init — no .kinora/
        let root = kinora_root(tmp.path());
        let err = store_kino(&root, params("markdown", b"x")).unwrap_err();
        assert!(matches!(err, StoreKinoError::KinoraMissing { .. }));
    }
}
