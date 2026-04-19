//! Compaction: promote hot-ledger events into a `root` kinograph version.
//!
//! `compact_root(kinora_root, root_name, …)` reads every event under
//! `.kinora/hot/`, picks the head version of each identity, and emits a
//! canonical `root`-kind kinograph whose entries inline the leaf view of
//! each owned kino. The blob is stored and `.kinora/roots/<name>` is
//! atomically rewritten to point at it.
//!
//! `compact_all(kinora_root, …)` is the batch driver: loads `config.styx`,
//! iterates every declared root in name order, and calls `compact_root`
//! per-root. Per-root failures don't short-circuit — clean roots still
//! advance to disk. Only a config read/parse failure surfaces as the
//! outer `Err`.
//!
//! Determinism: two independent devs running `compact_root` over the
//! same hot event set produce byte-identical root blobs.

use std::collections::BTreeMap;
use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::config::{Config, ConfigError};
use crate::event::{Event, EventError};
use crate::hash::{Hash, HashParseError};
use crate::kino::{store_kino, StoreKinoError, StoreKinoParams};
use crate::ledger::{Ledger, LedgerError};
use crate::paths::{config_path, root_pointer_path, roots_dir};
use crate::root::{RootEntry, RootError, RootKinograph};
use crate::store::{ContentStore, StoreError};

#[derive(Debug)]
pub enum CompactError {
    Io(io::Error),
    Ledger(LedgerError),
    Store(StoreError),
    Event(EventError),
    Root(RootError),
    StoreKino(StoreKinoError),
    Config(ConfigError),
    InvalidHash { value: String, err: HashParseError },
    MultipleHeads { id: String, heads: Vec<String> },
    NoHead { id: String },
    PriorEventMissing { version: String },
    InvalidPointer { path: PathBuf, body: String },
    InvalidRootName { name: String },
}

impl std::fmt::Display for CompactError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompactError::Io(e) => write!(f, "compact io error: {e}"),
            CompactError::Ledger(e) => write!(f, "{e}"),
            CompactError::Store(e) => write!(f, "{e}"),
            CompactError::Event(e) => write!(f, "{e}"),
            CompactError::Root(e) => write!(f, "{e}"),
            CompactError::StoreKino(e) => write!(f, "{e}"),
            CompactError::Config(e) => write!(f, "{e}"),
            CompactError::InvalidHash { value, err } => {
                write!(f, "invalid hash `{value}`: {err}")
            }
            CompactError::MultipleHeads { id, heads } => write!(
                f,
                "identity {id} has {} heads at compact time: {}",
                heads.len(),
                heads.join(", ")
            ),
            CompactError::NoHead { id } => write!(
                f,
                "identity {id} has no head (event graph cycle?)"
            ),
            CompactError::PriorEventMissing { version } => write!(
                f,
                "prior root pointer references version {version} but no matching event is in the ledger"
            ),
            CompactError::InvalidPointer { path, body } => write!(
                f,
                "root pointer at {} is not a 64-hex hash: {body:?}",
                path.display()
            ),
            CompactError::InvalidRootName { name } => write!(
                f,
                "invalid root name {name:?}: must be a single path component with no `/`, `\\`, or `..`"
            ),
        }
    }
}

impl std::error::Error for CompactError {}

impl From<io::Error> for CompactError {
    fn from(e: io::Error) -> Self {
        CompactError::Io(e)
    }
}

impl From<LedgerError> for CompactError {
    fn from(e: LedgerError) -> Self {
        CompactError::Ledger(e)
    }
}

impl From<StoreError> for CompactError {
    fn from(e: StoreError) -> Self {
        CompactError::Store(e)
    }
}

impl From<EventError> for CompactError {
    fn from(e: EventError) -> Self {
        CompactError::Event(e)
    }
}

impl From<RootError> for CompactError {
    fn from(e: RootError) -> Self {
        CompactError::Root(e)
    }
}

impl From<StoreKinoError> for CompactError {
    fn from(e: StoreKinoError) -> Self {
        CompactError::StoreKino(e)
    }
}

impl From<ConfigError> for CompactError {
    fn from(e: ConfigError) -> Self {
        CompactError::Config(e)
    }
}

/// Inputs for a compact call. Mirrors the parts of `StoreKinoParams` that
/// the root-kino event also needs.
#[derive(Debug, Clone)]
pub struct CompactParams {
    pub author: String,
    pub provenance: String,
    pub ts: String,
}

#[derive(Debug, Clone)]
pub struct CompactResult {
    pub root_name: String,
    /// Content hash of the newly stored root version. `None` iff the call
    /// was a no-op (either nothing to promote, or the new bytes matched the
    /// prior version byte-for-byte).
    pub new_version: Option<Hash>,
    pub prior_version: Option<Hash>,
}

/// Validate that `root_name` is a single safe path component. Rejects
/// empty strings, names containing `/` or `\`, and `..` / `.`. The pointer
/// file lives at `.kinora/roots/<name>`, so a name with traversal pieces
/// could escape the dir — block it defensively even though the CLI layer
/// ought to hand us well-formed input.
pub fn validate_root_name(name: &str) -> Result<(), CompactError> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
    {
        return Err(CompactError::InvalidRootName { name: name.to_owned() });
    }
    Ok(())
}

/// Read the current root pointer file. Returns `None` when the file does
/// not yet exist (no compaction has happened for this root). The body is
/// expected to be exactly a 64-hex hash with no trailing whitespace.
pub fn read_root_pointer(
    kinora_root: &Path,
    root_name: &str,
) -> Result<Option<Hash>, CompactError> {
    validate_root_name(root_name)?;
    let path = root_pointer_path(kinora_root, root_name);
    match fs::read_to_string(&path) {
        Ok(body) => {
            let trimmed = body.trim_end_matches(['\r', '\n']);
            let hash = Hash::from_str(trimmed).map_err(|_| CompactError::InvalidPointer {
                path: path.clone(),
                body,
            })?;
            Ok(Some(hash))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(CompactError::Io(e)),
    }
}

/// Atomically write `.kinora/roots/<name>` with the given 64-hex hash
/// (no trailing newline). Uses tmp+rename so a crash mid-write never
/// leaves a truncated pointer.
fn write_root_pointer(
    kinora_root: &Path,
    root_name: &str,
    hash: &Hash,
) -> Result<(), CompactError> {
    let dir = roots_dir(kinora_root);
    fs::create_dir_all(&dir)?;
    let path = root_pointer_path(kinora_root, root_name);
    let tmp = dir.join(format!(".{root_name}.tmp"));
    fs::write(&tmp, hash.as_hex())?;
    fs::rename(&tmp, &path)?;
    Ok(())
}

/// Compute the root kinograph that would be produced from the given event
/// set: for each identity, pick the head and emit one entry. Errors if any
/// identity has multiple heads (forks must be resolved before compaction —
/// phase 3 introduces assign events for this).
///
/// Events of kind `root` are skipped: a root kinograph represents the state
/// of user content, not its own history (prior root versions are linked
/// through the event chain, not re-entered into each successor).
pub fn build_root(events: &[Event]) -> Result<RootKinograph, CompactError> {
    let mut by_id: BTreeMap<String, Vec<&Event>> = BTreeMap::new();
    for e in events {
        if !e.is_store_event() {
            continue;
        }
        if e.kind == "root" {
            continue;
        }
        by_id.entry(e.id.clone()).or_default().push(e);
    }

    let mut entries: Vec<RootEntry> = Vec::with_capacity(by_id.len());
    for (id, group) in by_id {
        let head = pick_head(&id, &group)?;
        entries.push(RootEntry::new(
            head.id.clone(),
            head.hash.clone(),
            head.kind.clone(),
            head.metadata.clone(),
        ));
    }

    Ok(RootKinograph { entries })
}

fn pick_head<'a>(id: &str, events: &[&'a Event]) -> Result<&'a Event, CompactError> {
    let referenced: HashSet<&str> = events
        .iter()
        .flat_map(|e| e.parents.iter().map(String::as_str))
        .collect();
    let heads: Vec<&Event> = events
        .iter()
        .copied()
        .filter(|e| !referenced.contains(e.hash.as_str()))
        .collect();
    match heads.as_slice() {
        [only] => Ok(*only),
        [] => Err(CompactError::NoHead { id: id.to_owned() }),
        many => Err(CompactError::MultipleHeads {
            id: id.to_owned(),
            heads: many.iter().map(|e| e.hash.clone()).collect(),
        }),
    }
}

/// Run a compaction pass for the named root.
///
/// Genesis (no prior pointer): stores the new root as a birth event (`id`
/// auto-set to the blob hash, empty `parents`).
/// Subsequent: stores the new root as a version event whose `id` matches the
/// prior root's id and `parents` lists the prior version hash.
///
/// No-op: returns `new_version = None` when either
///  - no prior pointer exists AND there are no hot events to promote, or
///  - a prior pointer exists AND the fresh canonical bytes match it.
pub fn compact_root(
    kinora_root: &Path,
    root_name: &str,
    params: CompactParams,
) -> Result<CompactResult, CompactError> {
    validate_root_name(root_name)?;
    let prior_version = read_root_pointer(kinora_root, root_name)?;

    let ledger = Ledger::new(kinora_root);
    let events = ledger.read_all_events()?;

    let root = build_root(&events)?;
    let new_bytes = root.to_styx()?.into_bytes();

    if let Some(prior) = &prior_version {
        let prior_bytes = ContentStore::new(kinora_root).read(prior)?;
        if prior_bytes == new_bytes {
            return Ok(CompactResult {
                root_name: root_name.to_owned(),
                new_version: None,
                prior_version,
            });
        }
    } else if events.is_empty() {
        return Ok(CompactResult {
            root_name: root_name.to_owned(),
            new_version: None,
            prior_version,
        });
    }

    let (id, parents) = match &prior_version {
        Some(prior) => {
            let prior_event = events
                .iter()
                .find(|e| e.hash == prior.as_hex())
                .ok_or_else(|| CompactError::PriorEventMissing {
                    version: prior.as_hex().to_owned(),
                })?;
            (Some(prior_event.id.clone()), vec![prior.as_hex().to_owned()])
        }
        None => (None, vec![]),
    };

    let stored = store_kino(
        kinora_root,
        StoreKinoParams {
            kind: "root".into(),
            content: new_bytes,
            author: params.author,
            provenance: params.provenance,
            ts: params.ts,
            metadata: BTreeMap::new(),
            id,
            parents,
        },
    )?;

    let new_hash =
        Hash::from_str(&stored.event.hash).map_err(|err| CompactError::InvalidHash {
            value: stored.event.hash.clone(),
            err,
        })?;
    write_root_pointer(kinora_root, root_name, &new_hash)?;

    Ok(CompactResult {
        root_name: root_name.to_owned(),
        new_version: Some(new_hash),
        prior_version,
    })
}

/// One entry in the batch report produced by `compact_all`.
///
/// The outer tuple pairs the root's declared name with the per-root
/// outcome. A per-root `Err` surfaces the specific failure (e.g. a fork
/// on one root) without aborting the batch.
pub type CompactAllEntry = (String, Result<CompactResult, CompactError>);

/// Compact every root declared in `config.styx`, in name order.
///
/// Reads the config once, then calls `compact_root` per declared root.
/// Per-root errors are collected into the returned `Vec` — they don't
/// short-circuit the batch, so clean roots still advance to disk even
/// when a sibling root is in a failing state (e.g. a fork).
///
/// The outer `Result::Err` is reserved for pre-iteration failures:
/// config file missing, unreadable, or unparseable.
pub fn compact_all(
    kinora_root: &Path,
    params: CompactParams,
) -> Result<Vec<CompactAllEntry>, CompactError> {
    let cfg_path = config_path(kinora_root);
    let cfg_text = fs::read_to_string(&cfg_path)?;
    let config = Config::from_styx(&cfg_text)?;

    let mut out: Vec<CompactAllEntry> = Vec::with_capacity(config.roots.len());
    for name in config.roots.keys() {
        let result = compact_root(kinora_root, name, params.clone());
        out.push((name.clone(), result));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::init::init;
    use crate::kino::{store_kino, StoreKinoParams};
    use crate::paths::kinora_root;
    use tempfile::TempDir;

    fn setup() -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        init(tmp.path(), "https://example.com/x.git").unwrap();
        let root = kinora_root(tmp.path());
        (tmp, root)
    }

    fn params(author: &str, ts: &str) -> CompactParams {
        CompactParams {
            author: author.into(),
            provenance: "compact-test".into(),
            ts: ts.into(),
        }
    }

    fn store_md(root: &Path, content: &[u8], name: &str, ts: &str) -> Event {
        let stored = store_kino(
            root,
            StoreKinoParams {
                kind: "markdown".into(),
                content: content.to_vec(),
                author: "yj".into(),
                provenance: "compact-test".into(),
                ts: ts.into(),
                metadata: BTreeMap::from([("name".into(), name.into())]),
                id: None,
                parents: vec![],
            },
        )
        .unwrap();
        stored.event
    }

    #[test]
    fn genesis_produces_root_with_empty_parents() {
        let (_t, root) = setup();
        store_md(&root, b"a", "a", "2026-04-19T10:00:00Z");
        store_md(&root, b"b", "b", "2026-04-19T10:00:01Z");

        let result =
            compact_root(&root, "main", params("yj", "2026-04-19T10:00:02Z")).unwrap();
        let hash = result.new_version.expect("new version on genesis");
        assert!(result.prior_version.is_none());

        let events = Ledger::new(&root).read_all_events().unwrap();
        let root_event = events.iter().find(|e| e.kind == "root").unwrap();
        assert_eq!(root_event.hash, hash.as_hex());
        assert!(root_event.parents.is_empty(), "genesis has empty parents");
        assert_eq!(root_event.id, root_event.hash, "genesis id == hash");
    }

    #[test]
    fn subsequent_compaction_links_parent_and_bumps_version() {
        let (_t, root) = setup();
        store_md(&root, b"v1", "doc", "2026-04-19T10:00:00Z");

        let first = compact_root(&root, "main", params("yj", "2026-04-19T10:00:01Z"))
            .unwrap();
        let prior = first.new_version.unwrap();

        // Add a second kino so the second root differs.
        store_md(&root, b"second", "other", "2026-04-19T10:00:02Z");

        let second = compact_root(&root, "main", params("yj", "2026-04-19T10:00:03Z"))
            .unwrap();
        assert_eq!(second.prior_version.as_ref(), Some(&prior));
        let new = second.new_version.expect("new version after update");
        assert_ne!(new, prior, "version hash should differ after bump");

        let events = Ledger::new(&root).read_all_events().unwrap();
        let new_root_event = events
            .iter()
            .find(|e| e.kind == "root" && e.hash == new.as_hex())
            .unwrap();
        assert_eq!(new_root_event.parents, vec![prior.as_hex().to_owned()]);
        // Identity carried forward from the genesis root.
        let genesis_event = events
            .iter()
            .find(|e| e.kind == "root" && e.hash == prior.as_hex())
            .unwrap();
        assert_eq!(new_root_event.id, genesis_event.id);
    }

    #[test]
    fn compact_is_no_op_when_nothing_new() {
        let (_t, root) = setup();
        store_md(&root, b"one", "only", "2026-04-19T10:00:00Z");
        let first = compact_root(&root, "main", params("yj", "2026-04-19T10:00:01Z"))
            .unwrap();
        let first_version = first.new_version.unwrap();

        let pointer_before = fs::read(root_pointer_path(&root, "main")).unwrap();

        // No new user events; different ts on the compact itself.
        let second = compact_root(&root, "main", params("yj", "2026-04-19T10:05:00Z"))
            .unwrap();
        assert!(second.new_version.is_none(), "should be no-op");
        assert_eq!(second.prior_version.unwrap(), first_version);

        let pointer_after = fs::read(root_pointer_path(&root, "main")).unwrap();
        assert_eq!(pointer_before, pointer_after, "pointer unchanged on no-op");
    }

    #[test]
    fn compact_ignores_non_store_events() {
        // A hand-forged non-store event in the hot ledger must not appear
        // in the compacted root. Compact only sees content-track events.
        let (_t, root) = setup();
        store_md(&root, b"real", "doc", "2026-04-19T10:00:00Z");

        // Forge a bogus event; if compact didn't filter, it would try to
        // read its hash from the content store and fail.
        let forged = Event {
            event_kind: "assign".into(),
            kind: "assign".into(),
            id: "cc".repeat(32),
            hash: "dd".repeat(32),
            parents: vec![],
            ts: "2026-04-19T10:00:00Z".into(),
            author: "yj".into(),
            provenance: "test".into(),
            metadata: BTreeMap::new(),
        };
        Ledger::new(&root).write_event(&forged).unwrap();

        let result = compact_root(&root, "main", params("yj", "2026-04-19T10:00:05Z"))
            .unwrap();
        let hash = result.new_version.expect("expected compaction to succeed");
        let bytes = ContentStore::new(&root).read(&hash).unwrap();
        let kinograph = RootKinograph::parse(&bytes).unwrap();
        assert!(
            kinograph.entries.iter().all(|k| k.id != forged.id),
            "forged non-store event leaked into root kinograph"
        );
    }

    #[test]
    fn compact_with_no_events_and_no_prior_is_no_op() {
        let (_t, root) = setup();
        let result = compact_root(&root, "main", params("yj", "2026-04-19T10:00:00Z"))
            .unwrap();
        assert!(result.new_version.is_none());
        assert!(result.prior_version.is_none());
        assert!(!root_pointer_path(&root, "main").exists());
    }

    #[test]
    fn two_independent_compactions_produce_byte_identical_root_blobs() {
        // Run the same logical compaction in two fresh repos with different
        // compact author/ts/provenance — the root blob (content bytes) must
        // be byte-identical because it's derived purely from the user events.
        let mk = |root: &Path| {
            store_md(root, b"alpha", "a", "2026-04-19T10:00:00Z");
            store_md(root, b"beta", "b", "2026-04-19T10:00:01Z");
            store_md(root, b"gamma", "c", "2026-04-19T10:00:02Z");
        };

        let (_t1, root1) = setup();
        mk(&root1);
        let r1 =
            compact_root(&root1, "main", params("alice", "2026-04-19T10:00:03Z"))
                .unwrap()
                .new_version
                .unwrap();

        let (_t2, root2) = setup();
        mk(&root2);
        let r2 = compact_root(
            &root2,
            "main",
            CompactParams {
                author: "bob".into(),
                provenance: "somewhere-else".into(),
                ts: "2026-04-20T11:11:11Z".into(),
            },
        )
        .unwrap()
        .new_version
        .unwrap();

        let blob1 = ContentStore::new(&root1).read(&r1).unwrap();
        let blob2 = ContentStore::new(&root2).read(&r2).unwrap();
        assert_eq!(blob1, blob2, "root blob content must match byte-for-byte");
        assert_eq!(r1, r2, "therefore the content hashes match too");
    }

    #[test]
    fn root_entries_are_sorted_by_id() {
        let (_t, root) = setup();
        let a = store_md(&root, b"aa", "n1", "2026-04-19T10:00:00Z");
        let b = store_md(&root, b"bb", "n2", "2026-04-19T10:00:01Z");
        let c = store_md(&root, b"cc", "n3", "2026-04-19T10:00:02Z");

        let result = compact_root(&root, "main", params("yj", "2026-04-19T10:00:10Z"))
            .unwrap();
        let blob = ContentStore::new(&root)
            .read(&result.new_version.unwrap())
            .unwrap();
        let parsed = RootKinograph::parse(&blob).unwrap();
        let ids: Vec<_> = parsed.entries.iter().map(|e| e.id.clone()).collect();
        let mut sorted = vec![a.id, b.id, c.id];
        sorted.sort();
        assert_eq!(ids, sorted, "entries must be sorted by id");
    }

    #[test]
    fn pointer_file_is_exactly_64_hex_no_trailing_whitespace() {
        let (_t, root) = setup();
        store_md(&root, b"x", "x", "2026-04-19T10:00:00Z");
        let result = compact_root(&root, "main", params("yj", "2026-04-19T10:00:01Z"))
            .unwrap();
        let hash = result.new_version.unwrap();
        let pointer = fs::read_to_string(root_pointer_path(&root, "main")).unwrap();
        assert_eq!(
            pointer,
            hash.as_hex(),
            "pointer must be exactly the hash with no trailing whitespace/newline"
        );
        assert_eq!(pointer.len(), 64);
    }

    #[test]
    fn version_bump_keeps_three_entries_with_one_bumped() {
        // Store 3 kinos → compact (3 entries). Then update one to v2 and
        // compact again — root should still have 3 entries, with one
        // entry's `version` bumped to the v2 hash.
        let (_t, root) = setup();
        let a = store_md(&root, b"a", "a", "2026-04-19T10:00:00Z");
        let b = store_md(&root, b"b", "b", "2026-04-19T10:00:01Z");
        let c = store_md(&root, b"c", "c", "2026-04-19T10:00:02Z");

        let first = compact_root(&root, "main", params("yj", "2026-04-19T10:00:03Z"))
            .unwrap();
        let first_blob = ContentStore::new(&root)
            .read(&first.new_version.unwrap())
            .unwrap();
        let first_root = RootKinograph::parse(&first_blob).unwrap();
        assert_eq!(first_root.entries.len(), 3);

        // v2 of `b`
        let v2 = store_kino(
            &root,
            StoreKinoParams {
                kind: "markdown".into(),
                content: b"b2".to_vec(),
                author: "yj".into(),
                provenance: "compact-test".into(),
                ts: "2026-04-19T10:00:10Z".into(),
                metadata: BTreeMap::from([("name".into(), "b".into())]),
                id: Some(b.id.clone()),
                parents: vec![b.hash.clone()],
            },
        )
        .unwrap();

        let second = compact_root(&root, "main", params("yj", "2026-04-19T10:00:11Z"))
            .unwrap();
        let second_blob = ContentStore::new(&root)
            .read(&second.new_version.unwrap())
            .unwrap();
        let second_root = RootKinograph::parse(&second_blob).unwrap();
        assert_eq!(second_root.entries.len(), 3);

        let ids: Vec<_> = second_root.entries.iter().map(|e| e.id.clone()).collect();
        let mut expected = vec![a.id.clone(), b.id.clone(), c.id.clone()];
        expected.sort();
        assert_eq!(ids, expected);

        let bumped = second_root
            .entries
            .iter()
            .find(|e| e.id == b.id)
            .unwrap();
        assert_eq!(bumped.version, v2.event.hash, "b's version bumped to v2");

        let unchanged = second_root
            .entries
            .iter()
            .find(|e| e.id == a.id)
            .unwrap();
        assert_eq!(unchanged.version, a.hash, "a's version unchanged");
    }

    #[test]
    fn read_root_pointer_returns_none_when_absent() {
        let (_t, root) = setup();
        assert!(read_root_pointer(&root, "main").unwrap().is_none());
    }

    #[test]
    fn read_root_pointer_rejects_invalid_body() {
        let (_t, root) = setup();
        fs::create_dir_all(roots_dir(&root)).unwrap();
        fs::write(root_pointer_path(&root, "main"), "not-a-hash").unwrap();
        let err = read_root_pointer(&root, "main").unwrap_err();
        assert!(matches!(err, CompactError::InvalidPointer { .. }), "got: {err:?}");
    }

    #[test]
    fn read_root_pointer_trims_trailing_newline() {
        // Be forgiving of manually-edited pointer files that ended up with a
        // trailing LF or CRLF — we still accept them as valid hashes.
        let (_t, root) = setup();
        fs::create_dir_all(roots_dir(&root)).unwrap();
        let hash = "ab".repeat(32);
        fs::write(root_pointer_path(&root, "main"), format!("{hash}\n")).unwrap();
        let got = read_root_pointer(&root, "main").unwrap().unwrap();
        assert_eq!(got.as_hex(), hash);
    }

    #[test]
    fn invalid_root_name_rejected() {
        let (_t, root) = setup();
        for name in ["", ".", "..", "a/b", "dir/sub", "back\\slash"] {
            let err =
                compact_root(&root, name, params("yj", "2026-04-19T10:00:00Z")).unwrap_err();
            assert!(
                matches!(err, CompactError::InvalidRootName { .. }),
                "name {name:?} not rejected: {err:?}"
            );
        }
    }

    #[test]
    fn no_head_reported_distinctly_from_multiple_heads() {
        // Manufacture a degenerate event set where every event is someone's
        // parent — no head exists. Since store_kino's validator rejects
        // self-parents and missing-parents, construct events by hand and
        // feed them to `build_root` directly.
        let make = |hash: &str, parents: Vec<String>| Event::new_store(
            "markdown".into(),
            "id".into(),
            hash.into(),
            parents,
            "t".into(),
            "a".into(),
            "p".into(),
            BTreeMap::new(),
        );
        let a = make(&"aa".repeat(32), vec!["bb".repeat(32)]);
        let b = make(&"bb".repeat(32), vec!["aa".repeat(32)]);
        let err = build_root(&[a, b]).unwrap_err();
        assert!(matches!(err, CompactError::NoHead { .. }), "got: {err:?}");
    }

    #[test]
    fn fork_rejected_as_multiple_heads() {
        // Two sibling versions off the same parent → fork. Compaction must
        // refuse; assign events (phase 3) are the supported way to pick a
        // winner.
        let (_t, root) = setup();
        let birth = store_md(&root, b"v1", "doc", "2026-04-19T10:00:00Z");

        for (content, ts) in [
            (b"left" as &[u8], "2026-04-19T10:00:01Z"),
            (b"right", "2026-04-19T10:00:02Z"),
        ] {
            store_kino(
                &root,
                StoreKinoParams {
                    kind: "markdown".into(),
                    content: content.to_vec(),
                    author: "yj".into(),
                    provenance: "compact-test".into(),
                    ts: ts.into(),
                    metadata: BTreeMap::from([("name".into(), "doc".into())]),
                    id: Some(birth.id.clone()),
                    parents: vec![birth.hash.clone()],
                },
            )
            .unwrap();
        }

        let err =
            compact_root(&root, "main", params("yj", "2026-04-19T10:00:10Z")).unwrap_err();
        assert!(matches!(err, CompactError::MultipleHeads { .. }), "got: {err:?}");
    }

    // ------------------------------------------------------------------
    // compact_all (batch driver)
    // ------------------------------------------------------------------

    fn write_config(kin_root: &Path, body: &str) {
        fs::write(config_path(kin_root), body).unwrap();
    }

    #[test]
    fn compact_all_iterates_every_declared_root_in_name_order() {
        let (_t, root) = setup();
        // init writes a config with just `inbox`. Overwrite with three roots
        // listed out of alphabetical order in the file — compact_all should
        // normalize to sorted order.
        write_config(
            &root,
            r#"repo-url "https://example.com/x.git"
roots {
  zeta { policy "never" }
  alpha { policy "never" }
  main { policy "never" }
}
"#,
        );

        store_md(&root, b"x", "x", "2026-04-19T10:00:00Z");

        let entries = compact_all(&root, params("yj", "2026-04-19T10:00:01Z")).unwrap();
        let names: Vec<_> = entries.iter().map(|(n, _)| n.clone()).collect();
        // `inbox` is auto-provisioned by Config::from_styx when absent.
        assert_eq!(names, vec!["alpha", "inbox", "main", "zeta"]);
        assert!(
            entries.iter().all(|(_, r)| r.is_ok()),
            "every root should have compacted cleanly: {entries:?}"
        );
    }

    #[test]
    fn compact_all_per_root_errors_do_not_short_circuit_clean_roots() {
        let (_t, root) = setup();
        write_config(
            &root,
            r#"repo-url "https://example.com/x.git"
roots {
  main { policy "never" }
  forked { policy "never" }
  clean { policy "never" }
}
"#,
        );

        // Pre-populate `forked`'s pointer with a hash that isn't in the
        // content store — compact_root will fail to read the prior blob
        // for byte-comparison. main and clean are untouched and must
        // still advance to disk.
        fs::create_dir_all(roots_dir(&root)).unwrap();
        let bogus_hash = "ff".repeat(32);
        fs::write(root_pointer_path(&root, "forked"), &bogus_hash).unwrap();

        store_md(&root, b"x", "x", "2026-04-19T10:00:00Z");

        let entries = compact_all(&root, params("yj", "2026-04-19T10:00:01Z")).unwrap();
        let by_name: std::collections::HashMap<_, _> = entries
            .iter()
            .map(|(n, r)| (n.clone(), r))
            .collect();

        assert!(by_name["main"].is_ok(), "main: {:?}", by_name["main"]);
        assert!(by_name["clean"].is_ok(), "clean: {:?}", by_name["clean"]);
        assert!(
            by_name["forked"].is_err(),
            "forked should surface as Err: {:?}",
            by_name["forked"]
        );

        // main pointer advanced to disk despite the sibling failure.
        assert!(root_pointer_path(&root, "main").is_file());
        assert!(root_pointer_path(&root, "clean").is_file());
    }

    #[test]
    fn compact_all_surfaces_config_errors_as_outer_err() {
        let (_t, root) = setup();
        // Overwrite with an unparseable config.
        write_config(&root, "this is not valid styx {{{");
        let err = compact_all(&root, params("yj", "2026-04-19T10:00:00Z")).unwrap_err();
        assert!(
            matches!(err, CompactError::Config(_)),
            "config parse failure should be outer Err: {err:?}"
        );
    }

    #[test]
    fn compact_all_surfaces_missing_config_as_outer_err() {
        let (_t, root) = setup();
        fs::remove_file(config_path(&root)).unwrap();
        let err = compact_all(&root, params("yj", "2026-04-19T10:00:00Z")).unwrap_err();
        assert!(
            matches!(err, CompactError::Io(_)),
            "missing config.styx should be outer Err: {err:?}"
        );
    }

    #[test]
    fn compact_all_emits_no_op_entry_when_root_has_nothing_to_promote() {
        // Default init config has only `inbox`. No hot events → compact_all
        // should still visit inbox and emit a no-op entry.
        let (_t, root) = setup();
        let entries = compact_all(&root, params("yj", "2026-04-19T10:00:00Z")).unwrap();
        assert_eq!(entries.len(), 1);
        let (name, result) = &entries[0];
        assert_eq!(name, "inbox");
        let res = result.as_ref().unwrap();
        assert!(res.new_version.is_none());
        assert!(res.prior_version.is_none());
    }
}
