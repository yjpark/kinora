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
use std::collections::BTreeSet;
use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::assign::{AssignError, AssignEvent, EVENT_KIND_ASSIGN};
use crate::config::{Config, ConfigError};
use crate::event::{Event, EventError};
use crate::hash::{Hash, HashParseError};
use crate::kino::{store_kino, StoreKinoError, StoreKinoParams};
use crate::ledger::{Ledger, LedgerError};
use crate::paths::{config_path, root_pointer_path, roots_dir};
use crate::root::{RootEntry, RootError, RootKinograph};
use crate::store::{ContentStore, StoreError};

/// A single live assign candidate surfaced in `AmbiguousAssign` so callers
/// (notably the CLI) can render the D2 resolution hint without re-loading
/// the hot ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignCandidate {
    pub event_hash: String,
    pub target_root: String,
    pub author: String,
    pub ts: String,
}

#[derive(Debug)]
pub enum CompactError {
    Io(io::Error),
    Ledger(LedgerError),
    Store(StoreError),
    Event(EventError),
    Root(RootError),
    StoreKino(StoreKinoError),
    Config(ConfigError),
    Assign(AssignError),
    InvalidHash { value: String, err: HashParseError },
    MultipleHeads { id: String, heads: Vec<String> },
    NoHead { id: String },
    PriorEventMissing { version: String },
    InvalidPointer { path: PathBuf, body: String },
    InvalidRootName { name: String },
    /// A kino has two or more live (non-superseded) assign events pointing
    /// at it. The compact cannot decide ownership; the user must author a
    /// tie-breaking assign whose `supersedes` list names all candidates.
    AmbiguousAssign { kino_id: String, candidates: Vec<AssignCandidate> },
    /// A live assign references a root name that is not declared in
    /// `config.styx`. Raised during `compact_root` regardless of which
    /// root is currently being compacted — an undeclared target is a
    /// config/user error that must be fixed globally.
    UnknownRoot { name: String, event_hash: String },
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
            CompactError::Assign(e) => write!(f, "{e}"),
            CompactError::AmbiguousAssign { kino_id, candidates } => write!(
                f,
                "ambiguous assigns for kino {kino_id}: {} live candidates",
                candidates.len()
            ),
            CompactError::UnknownRoot { name, event_hash } => write!(
                f,
                "unknown root `{name}` referenced by assign event {event_hash}"
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

impl From<AssignError> for CompactError {
    fn from(e: AssignError) -> Self {
        CompactError::Assign(e)
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

/// Compute the root kinograph that would be produced for `root_name` from
/// the given event set.
///
/// Routing rule per kino id:
/// - zero live assigns → the kino routes to `inbox` (phase-3 default)
/// - one live assign → the kino routes to that assign's `target_root`; if
///   the target is not in `declared_roots`, `UnknownRoot` is raised.
/// - two or more live assigns → `AmbiguousAssign` surfaces all candidates
///
/// Only kinos routed to `root_name` are included in the returned kinograph.
/// Errors from the routing pass (`AmbiguousAssign`, `UnknownRoot`) bubble up
/// regardless of which root is being compacted — an undeclared target is a
/// global config/user problem that needs fixing before any root is clean.
///
/// Events of kind `root` are skipped: a root kinograph represents the state
/// of user content, not its own history.
pub fn build_root(
    events: &[Event],
    root_name: &str,
    declared_roots: &BTreeSet<String>,
) -> Result<RootKinograph, CompactError> {
    let live_assigns = collect_live_assigns(events)?;

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
        let target = kino_target_root(&id, &live_assigns, declared_roots)?;
        let target_name = target.as_deref().unwrap_or("inbox");
        if target_name != root_name {
            continue;
        }
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

/// Collect the live assign set from the hot event stream.
///
/// An assign is **live** iff its event hash is not named in any other
/// assign's `supersedes` list. Supersession is applied transitively via this
/// single-pass rule: if A←B←C, then A is in some supersedes list (B's), and
/// B is in some supersedes list (C's), so only C is live.
///
/// Returns `(event_hash, AssignEvent)` pairs so the caller can surface the
/// persistent assign identity in error payloads without re-hashing.
fn collect_live_assigns(
    events: &[Event],
) -> Result<Vec<(Hash, AssignEvent)>, CompactError> {
    let mut all: Vec<(Hash, AssignEvent)> = Vec::new();
    for e in events {
        if e.event_kind != EVENT_KIND_ASSIGN {
            continue;
        }
        let hash = e.event_hash()?;
        let a = AssignEvent::from_event(e)?;
        all.push((hash, a));
    }
    let superseded: HashSet<String> = all
        .iter()
        .flat_map(|(_, a)| a.supersedes.iter().cloned())
        .collect();
    Ok(all
        .into_iter()
        .filter(|(h, _)| !superseded.contains(h.as_hex()))
        .collect())
}

/// Decide which root a single kino belongs to based on its live assigns.
///
/// - `Ok(Some(target))`: exactly one live assign pins the kino to `target`.
///   `target` is guaranteed to be present in `declared_roots`.
/// - `Ok(None)`: no live assigns touch this kino — caller treats this as
///   default inbox routing.
/// - `Err(AmbiguousAssign | UnknownRoot)`: surface the failure with enough
///   detail for the CLI to render the D2 resolution hint.
fn kino_target_root(
    kino_id: &str,
    live_assigns: &[(Hash, AssignEvent)],
    declared_roots: &BTreeSet<String>,
) -> Result<Option<String>, CompactError> {
    let mine: Vec<&(Hash, AssignEvent)> = live_assigns
        .iter()
        .filter(|(_, a)| a.kino_id == kino_id)
        .collect();
    match mine.len() {
        0 => Ok(None),
        1 => {
            let (h, a) = mine[0];
            if !declared_roots.contains(&a.target_root) {
                return Err(CompactError::UnknownRoot {
                    name: a.target_root.clone(),
                    event_hash: h.as_hex().to_owned(),
                });
            }
            Ok(Some(a.target_root.clone()))
        }
        _ => {
            let candidates = mine
                .iter()
                .map(|(h, a)| AssignCandidate {
                    event_hash: h.as_hex().to_owned(),
                    target_root: a.target_root.clone(),
                    author: a.author.clone(),
                    ts: a.ts.clone(),
                })
                .collect();
            Err(CompactError::AmbiguousAssign {
                kino_id: kino_id.to_owned(),
                candidates,
            })
        }
    }
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

    let cfg_path = config_path(kinora_root);
    let cfg_text = fs::read_to_string(&cfg_path)?;
    let config = Config::from_styx(&cfg_text)?;
    let declared_roots: BTreeSet<String> = config.roots.keys().cloned().collect();

    let ledger = Ledger::new(kinora_root);
    let events = ledger.read_all_events()?;

    let root = build_root(&events, root_name, &declared_roots)?;
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
    } else if root.entries.is_empty() {
        // No kinos are routed here and no prior pointer exists — nothing to
        // materialize. Previously keyed on `events.is_empty()`; now that
        // routing excludes unrelated roots, key on the built kinograph's
        // emptiness so a root with zero assigns still no-ops cleanly.
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
    use crate::assign::{AssignEvent, EVENT_KIND_ASSIGN};
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
            compact_root(&root, "inbox", params("yj", "2026-04-19T10:00:02Z")).unwrap();
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

        let first = compact_root(&root, "inbox", params("yj", "2026-04-19T10:00:01Z"))
            .unwrap();
        let prior = first.new_version.unwrap();

        // Add a second kino so the second root differs.
        store_md(&root, b"second", "other", "2026-04-19T10:00:02Z");

        let second = compact_root(&root, "inbox", params("yj", "2026-04-19T10:00:03Z"))
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
        let first = compact_root(&root, "inbox", params("yj", "2026-04-19T10:00:01Z"))
            .unwrap();
        let first_version = first.new_version.unwrap();

        let pointer_before = fs::read(root_pointer_path(&root, "inbox")).unwrap();

        // No new user events; different ts on the compact itself.
        let second = compact_root(&root, "inbox", params("yj", "2026-04-19T10:05:00Z"))
            .unwrap();
        assert!(second.new_version.is_none(), "should be no-op");
        assert_eq!(second.prior_version.unwrap(), first_version);

        let pointer_after = fs::read(root_pointer_path(&root, "inbox")).unwrap();
        assert_eq!(pointer_before, pointer_after, "pointer unchanged on no-op");
    }

    #[test]
    fn compact_ignores_non_store_events() {
        // A hand-forged non-store event in the hot ledger must not appear
        // in the compacted root. Compact only sees content-track events.
        let (_t, root) = setup();
        store_md(&root, b"real", "doc", "2026-04-19T10:00:00Z");

        // Forge an event with a future/unknown event_kind. It is neither a
        // store event (so it must not land as a RootEntry) nor an assign
        // (so it must not be interpreted as one either). Compact should
        // tolerate it and still produce a clean root.
        let forged = Event {
            event_kind: "future_kind".into(),
            kind: "something::else".into(),
            id: "cc".repeat(32),
            hash: "dd".repeat(32),
            parents: vec![],
            ts: "2026-04-19T10:00:00Z".into(),
            author: "yj".into(),
            provenance: "test".into(),
            metadata: BTreeMap::new(),
        };
        Ledger::new(&root).write_event(&forged).unwrap();

        let result = compact_root(&root, "inbox", params("yj", "2026-04-19T10:00:05Z"))
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
        let result = compact_root(&root, "inbox", params("yj", "2026-04-19T10:00:00Z"))
            .unwrap();
        assert!(result.new_version.is_none());
        assert!(result.prior_version.is_none());
        assert!(!root_pointer_path(&root, "inbox").exists());
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
            compact_root(&root1, "inbox", params("alice", "2026-04-19T10:00:03Z"))
                .unwrap()
                .new_version
                .unwrap();

        let (_t2, root2) = setup();
        mk(&root2);
        let r2 = compact_root(
            &root2,
            "inbox",
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

        let result = compact_root(&root, "inbox", params("yj", "2026-04-19T10:00:10Z"))
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
        let result = compact_root(&root, "inbox", params("yj", "2026-04-19T10:00:01Z"))
            .unwrap();
        let hash = result.new_version.unwrap();
        let pointer = fs::read_to_string(root_pointer_path(&root, "inbox")).unwrap();
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

        let first = compact_root(&root, "inbox", params("yj", "2026-04-19T10:00:03Z"))
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

        let second = compact_root(&root, "inbox", params("yj", "2026-04-19T10:00:11Z"))
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
        assert!(read_root_pointer(&root, "inbox").unwrap().is_none());
    }

    #[test]
    fn read_root_pointer_rejects_invalid_body() {
        let (_t, root) = setup();
        fs::create_dir_all(roots_dir(&root)).unwrap();
        fs::write(root_pointer_path(&root, "inbox"), "not-a-hash").unwrap();
        let err = read_root_pointer(&root, "inbox").unwrap_err();
        assert!(matches!(err, CompactError::InvalidPointer { .. }), "got: {err:?}");
    }

    #[test]
    fn read_root_pointer_trims_trailing_newline() {
        // Be forgiving of manually-edited pointer files that ended up with a
        // trailing LF or CRLF — we still accept them as valid hashes.
        let (_t, root) = setup();
        fs::create_dir_all(roots_dir(&root)).unwrap();
        let hash = "ab".repeat(32);
        fs::write(root_pointer_path(&root, "inbox"), format!("{hash}\n")).unwrap();
        let got = read_root_pointer(&root, "inbox").unwrap().unwrap();
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
        let declared: BTreeSet<String> = BTreeSet::from(["inbox".to_owned()]);
        let err = build_root(&[a, b], "inbox", &declared).unwrap_err();
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
            compact_root(&root, "inbox", params("yj", "2026-04-19T10:00:10Z")).unwrap_err();
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
        // for byte-comparison. main and clean each get an explicit assign
        // so they produce non-empty root kinographs and must still advance
        // to disk despite the sibling failure.
        fs::create_dir_all(roots_dir(&root)).unwrap();
        let bogus_hash = "ff".repeat(32);
        fs::write(root_pointer_path(&root, "forked"), &bogus_hash).unwrap();

        let km = store_md(&root, b"m", "m", "2026-04-19T10:00:00Z");
        let kc = store_md(&root, b"c", "c", "2026-04-19T10:00:00Z");
        write_assign_for(&root, &km.id, "main", vec![], "2026-04-19T10:00:01Z");
        write_assign_for(&root, &kc.id, "clean", vec![], "2026-04-19T10:00:02Z");

        let entries = compact_all(&root, params("yj", "2026-04-19T10:00:03Z")).unwrap();
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

    // ------------------------------------------------------------------
    // 7mou: compact consumes assigns + AmbiguousAssign + UnknownRoot
    // ------------------------------------------------------------------

    fn write_assign_for(
        kin: &Path,
        kino_id: &str,
        target_root: &str,
        supersedes: Vec<String>,
        ts: &str,
    ) -> Hash {
        let a = AssignEvent {
            kino_id: kino_id.to_owned(),
            target_root: target_root.to_owned(),
            supersedes,
            author: "yj".into(),
            ts: ts.to_owned(),
            provenance: "compact-test".into(),
        };
        let (h, _) = crate::assign::write_assign(kin, &a).unwrap();
        h
    }

    fn root_ids(kin: &Path, version: &Hash) -> Vec<String> {
        let bytes = ContentStore::new(kin).read(version).unwrap();
        let parsed = RootKinograph::parse(&bytes).unwrap();
        parsed.entries.into_iter().map(|e| e.id).collect()
    }

    #[test]
    fn single_live_assign_routes_kino_to_target_root_not_inbox() {
        let (_t, root) = setup();
        write_config(
            &root,
            r#"repo-url "https://example.com/x.git"
roots {
  rfcs { policy "never" }
}
"#,
        );

        let k = store_md(&root, b"spec", "spec", "2026-04-19T10:00:00Z");
        write_assign_for(&root, &k.id, "rfcs", vec![], "2026-04-19T10:00:01Z");

        // rfcs gets the kino; inbox does not.
        let rfcs_res =
            compact_root(&root, "rfcs", params("yj", "2026-04-19T10:00:02Z")).unwrap();
        let rfcs_ids = root_ids(&root, &rfcs_res.new_version.unwrap());
        assert_eq!(rfcs_ids, vec![k.id.clone()], "rfcs should own the kino");

        let inbox_res =
            compact_root(&root, "inbox", params("yj", "2026-04-19T10:00:03Z")).unwrap();
        assert!(
            inbox_res.new_version.is_none(),
            "inbox should be empty (no-op) since the kino is routed to rfcs"
        );
    }

    #[test]
    fn unassigned_kinos_default_to_inbox_not_main() {
        let (_t, root) = setup();
        write_config(
            &root,
            r#"repo-url "https://example.com/x.git"
roots {
  main { policy "never" }
}
"#,
        );

        let k = store_md(&root, b"x", "x", "2026-04-19T10:00:00Z");

        let inbox_res =
            compact_root(&root, "inbox", params("yj", "2026-04-19T10:00:01Z")).unwrap();
        let inbox_ids = root_ids(&root, &inbox_res.new_version.unwrap());
        assert_eq!(inbox_ids, vec![k.id.clone()], "unassigned kino should land in inbox");

        let main_res =
            compact_root(&root, "main", params("yj", "2026-04-19T10:00:02Z")).unwrap();
        assert!(
            main_res.new_version.is_none(),
            "main should be no-op; unassigned kinos do not implicitly land there"
        );
    }

    #[test]
    fn superseded_assign_is_not_live_superseder_wins() {
        let (_t, root) = setup();
        write_config(
            &root,
            r#"repo-url "https://example.com/x.git"
roots {
  rfcs { policy "never" }
  designs { policy "never" }
}
"#,
        );

        let k = store_md(&root, b"doc", "doc", "2026-04-19T10:00:00Z");
        let first = write_assign_for(&root, &k.id, "rfcs", vec![], "2026-04-19T10:00:01Z");
        // Reassign to designs, superseding the first.
        write_assign_for(
            &root,
            &k.id,
            "designs",
            vec![first.as_hex().to_owned()],
            "2026-04-19T10:00:02Z",
        );

        let designs_res =
            compact_root(&root, "designs", params("yj", "2026-04-19T10:00:03Z")).unwrap();
        let designs_ids = root_ids(&root, &designs_res.new_version.unwrap());
        assert_eq!(designs_ids, vec![k.id.clone()], "designs wins after supersede");

        let rfcs_res =
            compact_root(&root, "rfcs", params("yj", "2026-04-19T10:00:04Z")).unwrap();
        assert!(
            rfcs_res.new_version.is_none(),
            "rfcs should be no-op; its live assign was superseded"
        );
    }

    #[test]
    fn transitively_superseded_assign_only_terminal_superseder_is_live() {
        // A superseded by B, B superseded by C → only C counts.
        let (_t, root) = setup();
        write_config(
            &root,
            r#"repo-url "https://example.com/x.git"
roots {
  a { policy "never" }
  b { policy "never" }
  c { policy "never" }
}
"#,
        );

        let k = store_md(&root, b"doc", "doc", "2026-04-19T10:00:00Z");
        let ha = write_assign_for(&root, &k.id, "a", vec![], "2026-04-19T10:00:01Z");
        let hb = write_assign_for(
            &root,
            &k.id,
            "b",
            vec![ha.as_hex().to_owned()],
            "2026-04-19T10:00:02Z",
        );
        write_assign_for(
            &root,
            &k.id,
            "c",
            vec![hb.as_hex().to_owned()],
            "2026-04-19T10:00:03Z",
        );

        let c_res = compact_root(&root, "c", params("yj", "2026-04-19T10:00:04Z")).unwrap();
        let c_ids = root_ids(&root, &c_res.new_version.unwrap());
        assert_eq!(c_ids, vec![k.id.clone()]);

        let a_res = compact_root(&root, "a", params("yj", "2026-04-19T10:00:05Z")).unwrap();
        assert!(a_res.new_version.is_none(), "a's assign was superseded");

        let b_res = compact_root(&root, "b", params("yj", "2026-04-19T10:00:06Z")).unwrap();
        assert!(b_res.new_version.is_none(), "b's assign was superseded");
    }

    #[test]
    fn two_competing_live_assigns_raise_ambiguous_assign() {
        let (_t, root) = setup();
        write_config(
            &root,
            r#"repo-url "https://example.com/x.git"
roots {
  rfcs { policy "never" }
  designs { policy "never" }
}
"#,
        );

        let k = store_md(&root, b"doc", "doc", "2026-04-19T10:00:00Z");
        let h1 = write_assign_for(&root, &k.id, "rfcs", vec![], "2026-04-19T10:00:01Z");
        let h2 =
            write_assign_for(&root, &k.id, "designs", vec![], "2026-04-19T10:00:02Z");

        let err =
            compact_root(&root, "rfcs", params("yj", "2026-04-19T10:00:03Z")).unwrap_err();
        match err {
            CompactError::AmbiguousAssign { kino_id, candidates } => {
                assert_eq!(kino_id, k.id);
                assert_eq!(candidates.len(), 2, "should surface both live candidates");
                let hashes: HashSet<_> =
                    candidates.iter().map(|c| c.event_hash.clone()).collect();
                assert!(hashes.contains(h1.as_hex()));
                assert!(hashes.contains(h2.as_hex()));
                let targets: HashSet<_> =
                    candidates.iter().map(|c| c.target_root.clone()).collect();
                assert!(targets.contains("rfcs"));
                assert!(targets.contains("designs"));
            }
            other => panic!("expected AmbiguousAssign, got {other:?}"),
        }
    }

    #[test]
    fn assign_to_undeclared_root_raises_unknown_root() {
        let (_t, root) = setup();
        // Default config declares only `inbox` (auto-provisioned).
        let k = store_md(&root, b"doc", "doc", "2026-04-19T10:00:00Z");
        let h =
            write_assign_for(&root, &k.id, "madeup", vec![], "2026-04-19T10:00:01Z");

        let err =
            compact_root(&root, "inbox", params("yj", "2026-04-19T10:00:02Z")).unwrap_err();
        match err {
            CompactError::UnknownRoot { name, event_hash } => {
                assert_eq!(name, "madeup");
                assert_eq!(event_hash, h.as_hex());
            }
            other => panic!("expected UnknownRoot, got {other:?}"),
        }
    }

    #[test]
    fn cross_root_removal_kino_moves_from_main_to_rfcs() {
        let (_t, root) = setup();
        write_config(
            &root,
            r#"repo-url "https://example.com/x.git"
roots {
  main { policy "never" }
  rfcs { policy "never" }
}
"#,
        );

        // Pin the kino to `main` first.
        let k = store_md(&root, b"doc", "doc", "2026-04-19T10:00:00Z");
        write_assign_for(&root, &k.id, "main", vec![], "2026-04-19T10:00:01Z");

        let first = compact_all(&root, params("yj", "2026-04-19T10:00:02Z")).unwrap();
        let by_name: std::collections::HashMap<_, _> =
            first.into_iter().collect();
        let main_v1 = by_name["main"].as_ref().unwrap().new_version.clone().unwrap();
        let main_ids = root_ids(&root, &main_v1);
        assert_eq!(main_ids, vec![k.id.clone()], "main should initially own the kino");

        // Reassign to rfcs; main's last-assign hash is looked up from the
        // previous step's event stream via the ledger.
        let prior_assigns = Ledger::new(&root).read_all_events().unwrap();
        let prior_main_assign = prior_assigns
            .iter()
            .find(|e| e.event_kind == EVENT_KIND_ASSIGN)
            .unwrap();
        let supersedes_hash = prior_main_assign.event_hash().unwrap();
        write_assign_for(
            &root,
            &k.id,
            "rfcs",
            vec![supersedes_hash.as_hex().to_owned()],
            "2026-04-19T10:00:03Z",
        );

        // Re-compact both roots. Main should drop the kino; rfcs should gain it.
        let second = compact_all(&root, params("yj", "2026-04-19T10:00:04Z")).unwrap();
        let by_name: std::collections::HashMap<_, _> =
            second.into_iter().collect();

        let main_v2 = by_name["main"].as_ref().unwrap();
        let main_v2_hash = main_v2.new_version.as_ref().expect("main should bump");
        let main_ids_v2 = root_ids(&root, main_v2_hash);
        assert!(
            main_ids_v2.is_empty(),
            "main should no longer contain the kino after reassign, got {main_ids_v2:?}"
        );

        let rfcs_v1 = by_name["rfcs"].as_ref().unwrap();
        let rfcs_ids = root_ids(&root, &rfcs_v1.new_version.clone().unwrap());
        assert_eq!(rfcs_ids, vec![k.id.clone()], "rfcs should now own the kino");
    }

    #[test]
    fn ambiguous_assign_candidates_carry_author_and_ts() {
        // The rendered D2 hint needs author + ts per candidate; check the
        // CompactError payload carries them through.
        let (_t, root) = setup();
        write_config(
            &root,
            r#"repo-url "https://example.com/x.git"
roots {
  rfcs { policy "never" }
  designs { policy "never" }
}
"#,
        );
        let k = store_md(&root, b"x", "x", "2026-04-19T10:00:00Z");
        write_assign_for(&root, &k.id, "rfcs", vec![], "2026-04-19T10:00:01Z");
        write_assign_for(&root, &k.id, "designs", vec![], "2026-04-19T10:00:02Z");

        let err =
            compact_root(&root, "rfcs", params("yj", "2026-04-19T10:00:03Z")).unwrap_err();
        let candidates = match err {
            CompactError::AmbiguousAssign { candidates, .. } => candidates,
            other => panic!("expected AmbiguousAssign, got {other:?}"),
        };
        assert!(candidates.iter().all(|c| c.author == "yj"));
        let timestamps: HashSet<_> = candidates.iter().map(|c| c.ts.clone()).collect();
        assert!(timestamps.contains("2026-04-19T10:00:01Z"));
        assert!(timestamps.contains("2026-04-19T10:00:02Z"));
    }

    // ------------------------------------------------------------------
    // mngq: GC / prune / pin (per-policy)
    // ------------------------------------------------------------------

    use crate::paths::{hot_dir, hot_event_path};

    /// Replace the prior root pointer with a new root kinograph that pins
    /// the entry for `kino_id` to the given `version` hash. Writes the new
    /// blob to the content store, authors a root store event linked to the
    /// prior version via `parents`, and points `.kinora/roots/<name>` at
    /// the new version. Mirrors what a user hand-editing the root to add a
    /// pin would produce, so subsequent `compact_root` calls see the pin.
    fn overwrite_root_with_pin(
        kin: &Path,
        root_name: &str,
        kino_id: &str,
        pinned_version: &str,
        now: &str,
    ) -> Hash {
        let prior = read_root_pointer(kin, root_name).unwrap().expect("need prior root");
        let prior_bytes = ContentStore::new(kin).read(&prior).unwrap();
        let mut rk = RootKinograph::parse(&prior_bytes).unwrap();
        for e in rk.entries.iter_mut() {
            if e.id == kino_id {
                e.pin = true;
                e.version = pinned_version.to_owned();
            }
        }
        let bytes = rk.to_styx().unwrap().into_bytes();
        let events = Ledger::new(kin).read_all_events().unwrap();
        let prior_root_event = events
            .iter()
            .find(|e| e.hash == prior.as_hex())
            .expect("prior root event present");
        let stored = store_kino(
            kin,
            StoreKinoParams {
                kind: "root".into(),
                content: bytes,
                author: "pin-hack".into(),
                provenance: "test-pin".into(),
                ts: now.into(),
                metadata: BTreeMap::new(),
                id: Some(prior_root_event.id.clone()),
                parents: vec![prior.as_hex().to_owned()],
            },
        )
        .unwrap();
        let new_hash = Hash::from_str(&stored.event.hash).unwrap();
        write_root_pointer(kin, root_name, &new_hash).unwrap();
        new_hash
    }

    /// Count hot event files on disk.
    fn hot_event_count(kin: &Path) -> usize {
        let dir = hot_dir(kin);
        if !dir.exists() {
            return 0;
        }
        let mut n = 0;
        for shard in fs::read_dir(&dir).unwrap() {
            let shard = shard.unwrap();
            if !shard.file_type().unwrap().is_dir() {
                continue;
            }
            for entry in fs::read_dir(shard.path()).unwrap() {
                let p = entry.unwrap().path();
                if p.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                    n += 1;
                }
            }
        }
        n
    }

    /// True iff the hot event file for the given event exists.
    ///
    /// Hot events are keyed by `event.event_hash()` (BLAKE3 of the JSON line),
    /// NOT by `event.hash` (the content/blob hash). Tests that want to assert
    /// on ledger presence must use this helper rather than comparing
    /// `event.hash` against directory listings.
    fn hot_event_exists(kin: &Path, event: &Event) -> bool {
        let h = event.event_hash().unwrap();
        hot_event_path(kin, &h).is_file()
    }

    /// Store `n` successive versions of a kino under a single id chain.
    /// Returns the events in creation order (v1 first, v_n last).
    fn store_chain(root: &Path, kino_id: Option<String>, versions: &[(&[u8], &str)]) -> Vec<Event> {
        let mut out: Vec<Event> = Vec::new();
        let mut parents: Vec<String> = vec![];
        let mut id_override = kino_id;
        for (i, (content, ts)) in versions.iter().enumerate() {
            let name = format!("chain-{i}");
            let stored = store_kino(
                root,
                StoreKinoParams {
                    kind: "markdown".into(),
                    content: content.to_vec(),
                    author: "yj".into(),
                    provenance: "chain".into(),
                    ts: (*ts).into(),
                    metadata: BTreeMap::from([("name".into(), name)]),
                    id: id_override.take().or_else(|| out.first().map(|e| e.id.clone())),
                    parents: parents.clone(),
                },
            )
            .unwrap();
            parents = vec![stored.event.hash.clone()];
            out.push(stored.event);
        }
        out
    }

    // -- Root-entry GC under MaxAge + pin --------------------------------

    #[test]
    fn never_policy_keeps_root_entry_no_matter_how_old() {
        let (_t, root) = setup();
        write_config(
            &root,
            r#"repo-url "https://example.com/x.git"
roots {
  rfcs { policy "never" }
}
"#,
        );
        // An assign routes it to rfcs. Content ts is 2 years in the past.
        let k = store_md(&root, b"v", "v", "2024-04-19T10:00:00Z");
        write_assign_for(&root, &k.id, "rfcs", vec![], "2024-04-19T10:00:01Z");
        let res = compact_root(&root, "rfcs", params("yj", "2026-04-19T12:00:00Z")).unwrap();
        let ids = root_ids(&root, &res.new_version.unwrap());
        assert_eq!(ids, vec![k.id], "Never must not drop any entry");
    }

    #[test]
    fn max_age_policy_drops_old_unpinned_entry_but_keeps_recent() {
        let (_t, root) = setup();
        write_config(
            &root,
            r#"repo-url "https://example.com/x.git"
roots {
  rfcs { policy "7d" }
}
"#,
        );
        // Two kinos, both assigned to rfcs. One 8-days old (drop), one 6-days (keep).
        let old = store_md(&root, b"old", "old", "2026-04-11T10:00:00Z"); // 8d < now
        let fresh = store_md(&root, b"fresh", "fresh", "2026-04-13T10:00:00Z"); // 6d < now
        write_assign_for(&root, &old.id, "rfcs", vec![], "2026-04-11T10:00:01Z");
        write_assign_for(&root, &fresh.id, "rfcs", vec![], "2026-04-13T10:00:01Z");
        let res = compact_root(&root, "rfcs", params("yj", "2026-04-19T10:00:00Z")).unwrap();
        let ids = root_ids(&root, &res.new_version.unwrap());
        assert_eq!(
            ids,
            vec![fresh.id.clone()],
            "8-day-old entry should be dropped; 6-day-old kept"
        );
        assert!(!ids.contains(&old.id));
    }

    #[test]
    fn max_age_policy_pin_exempts_old_entry_from_drop() {
        let (_t, root) = setup();
        write_config(
            &root,
            r#"repo-url "https://example.com/x.git"
roots {
  rfcs { policy "7d" }
}
"#,
        );
        let old = store_md(&root, b"old", "old", "2026-04-05T10:00:00Z"); // 14d old
        write_assign_for(&root, &old.id, "rfcs", vec![], "2026-04-05T10:00:01Z");
        // First compact — produces a root with the entry. Then pin it.
        compact_root(&root, "rfcs", params("yj", "2026-04-11T10:00:00Z")).unwrap();
        overwrite_root_with_pin(&root, "rfcs", &old.id, &old.hash, "2026-04-11T10:00:01Z");
        // Second compact with a much later `now` — the 14-day-old entry
        // would normally drop, but pin should exempt it.
        let res = compact_root(&root, "rfcs", params("yj", "2026-04-19T10:00:00Z")).unwrap();
        let version = res.new_version.unwrap_or_else(|| res.prior_version.clone().unwrap());
        let ids = root_ids(&root, &version);
        assert_eq!(ids, vec![old.id], "pinned old entry must survive");
    }

    // -- Hot-ledger prune: MaxAge ----------------------------------------

    #[test]
    fn max_age_hot_ledger_prunes_events_older_than_policy() {
        let (_t, root) = setup();
        write_config(
            &root,
            r#"repo-url "https://example.com/x.git"
roots {
  rfcs { policy "7d" }
}
"#,
        );
        let old = store_md(&root, b"old", "old", "2026-04-05T10:00:00Z"); // 14d
        let fresh = store_md(&root, b"fresh", "fresh", "2026-04-18T10:00:00Z"); // 1d
        write_assign_for(&root, &old.id, "rfcs", vec![], "2026-04-05T10:00:01Z");
        write_assign_for(&root, &fresh.id, "rfcs", vec![], "2026-04-18T10:00:01Z");

        compact_root(&root, "rfcs", params("yj", "2026-04-19T10:00:00Z")).unwrap();
        // old's store event must have been pruned; fresh's retained.
        assert!(
            !hot_event_exists(&root, &old),
            "stale hot event should be gone"
        );
        assert!(
            hot_event_exists(&root, &fresh),
            "fresh hot event should survive"
        );
    }

    // -- Hot-ledger prune: KeepLastN -------------------------------------

    #[test]
    fn keep_last_n_keeps_only_n_most_recent_hot_events_per_kino() {
        let (_t, root) = setup();
        write_config(
            &root,
            r#"repo-url "https://example.com/x.git"
roots {
  rfcs { policy "keep-last-3" }
}
"#,
        );
        // Five versions of one kino. Oldest first.
        let chain = store_chain(
            &root,
            None,
            &[
                (b"v1", "2026-04-01T10:00:00Z"),
                (b"v2", "2026-04-02T10:00:00Z"),
                (b"v3", "2026-04-03T10:00:00Z"),
                (b"v4", "2026-04-04T10:00:00Z"),
                (b"v5", "2026-04-05T10:00:00Z"),
            ],
        );
        write_assign_for(&root, &chain[0].id, "rfcs", vec![], "2026-04-05T10:00:01Z");
        compact_root(&root, "rfcs", params("yj", "2026-04-19T10:00:00Z")).unwrap();

        // v3, v4, v5 survive; v1, v2 get pruned.
        assert!(!hot_event_exists(&root, &chain[0]), "v1 should be pruned");
        assert!(!hot_event_exists(&root, &chain[1]), "v2 should be pruned");
        assert!(hot_event_exists(&root, &chain[2]), "v3 should survive");
        assert!(hot_event_exists(&root, &chain[3]), "v4 should survive");
        assert!(hot_event_exists(&root, &chain[4]), "v5 should survive");
    }

    #[test]
    fn keep_last_n_pin_on_version_1_survives_plus_three_newest() {
        let (_t, root) = setup();
        write_config(
            &root,
            r#"repo-url "https://example.com/x.git"
roots {
  rfcs { policy "keep-last-3" }
}
"#,
        );
        let chain = store_chain(
            &root,
            None,
            &[
                (b"v1", "2026-04-01T10:00:00Z"),
                (b"v2", "2026-04-02T10:00:00Z"),
                (b"v3", "2026-04-03T10:00:00Z"),
                (b"v4", "2026-04-04T10:00:00Z"),
                (b"v5", "2026-04-05T10:00:00Z"),
            ],
        );
        write_assign_for(&root, &chain[0].id, "rfcs", vec![], "2026-04-05T10:00:01Z");
        compact_root(&root, "rfcs", params("yj", "2026-04-05T11:00:00Z")).unwrap();
        // Pin the root entry to v1 explicitly. This simulates a hand-edit.
        overwrite_root_with_pin(&root, "rfcs", &chain[0].id, &chain[0].hash, "2026-04-05T11:30:00Z");
        // Next compact runs the full KeepLastN(3) sweep.
        compact_root(&root, "rfcs", params("yj", "2026-04-19T10:00:00Z")).unwrap();
        // v1 pinned → survives; v3, v4, v5 are the 3 newest → survive; v2 pruned.
        assert!(hot_event_exists(&root, &chain[0]), "v1 pinned, must survive");
        assert!(!hot_event_exists(&root, &chain[1]), "v2 not in top-3, not pinned → pruned");
        assert!(hot_event_exists(&root, &chain[2]), "v3 in top-3, survives");
        assert!(hot_event_exists(&root, &chain[3]), "v4 in top-3, survives");
        assert!(hot_event_exists(&root, &chain[4]), "v5 in top-3, survives");
    }

    // -- Hot-ledger prune baseline: fresh events untouched ----------------

    #[test]
    fn fresh_hot_events_untouched_by_policy() {
        let (_t, root) = setup();
        write_config(
            &root,
            r#"repo-url "https://example.com/x.git"
roots {
  rfcs { policy "7d" }
}
"#,
        );
        // Two events, both under 7 days old.
        let a = store_md(&root, b"a", "a", "2026-04-18T10:00:00Z");
        let b = store_md(&root, b"b", "b", "2026-04-19T09:00:00Z");
        write_assign_for(&root, &a.id, "rfcs", vec![], "2026-04-18T10:00:01Z");
        write_assign_for(&root, &b.id, "rfcs", vec![], "2026-04-19T09:00:01Z");
        let count_before = hot_event_count(&root);
        compact_root(&root, "rfcs", params("yj", "2026-04-19T10:00:00Z")).unwrap();
        let count_after = hot_event_count(&root);
        // Compact adds a root event (+1) but must not drop the two user + two assign events.
        assert!(
            count_after >= count_before,
            "fresh events must survive: before={count_before}, after={count_after}"
        );
        assert!(hot_event_exists(&root, &a));
        assert!(hot_event_exists(&root, &b));
    }
}
