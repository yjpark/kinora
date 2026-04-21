//! Commit: promote staged-ledger events into a `root` kinograph version.
//!
//! `commit_root(kinora_root, root_name, …)` reads every event under
//! `.kinora/staged/`, picks the head version of each identity, and emits a
//! canonical `root`-kind kinograph whose entries inline the leaf view of
//! each owned kino. The blob is stored and `.kinora/roots/<name>` is
//! atomically rewritten to point at it.
//!
//! `commit_all(kinora_root, …)` is the batch driver: loads `config.styx`,
//! iterates every declared root in name order, and calls `commit_root`
//! per-root. Per-root failures don't short-circuit — clean roots still
//! advance to disk. Only a config read/parse failure surfaces as the
//! outer `Err`.
//!
//! Determinism: two independent devs running `commit_root` over the
//! same staged event set produce byte-identical root blobs.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::assign::{write_assign, AssignError, AssignEvent, EVENT_KIND_ASSIGN};
use crate::commit_archive::{
    serialize_archive, ArchiveError, ARCHIVE_CONTENT_KIND,
};
use crate::config::{Config, ConfigError, RootPolicy};
use crate::event::{Event, EventError};
use crate::hash::{Hash, HashParseError};
use crate::kino::{store_kino, StoreKinoError, StoreKinoParams};
use crate::kinograph::{Kinograph, KinographError};
use crate::ledger::{Ledger, LedgerError};
use crate::paths::{config_path, staged_event_path, root_pointer_path, roots_dir};
use crate::root::{RootEntry, RootError, RootKinograph};
use crate::store::{ContentStore, StoreError};

/// Name of the reserved root that holds per-commit archive kinos. Every
/// non-commits root produces one archive kino per commit that actually
/// promotes work; the archive is then assigned to this root so its own
/// kinograph doubles as the commit history for the repo.
///
/// Auto-provisioned by `Config::from_styx` with `RootPolicy::Never` —
/// archives are never dropped on age/window.
pub const COMMITS_ROOT: &str = "commits";

/// A single live assign candidate surfaced in `AmbiguousAssign` so callers
/// (notably the CLI) can render the D2 resolution hint without re-loading
/// the staged ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignCandidate {
    pub event_hash: String,
    pub target_root: String,
    pub author: String,
    pub ts: String,
}

#[derive(Debug, thiserror::Error)]
pub enum CommitError {
    #[error("commit io error: {0}")]
    Io(#[from] io::Error),
    #[error(transparent)]
    Ledger(#[from] LedgerError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Event(#[from] EventError),
    #[error(transparent)]
    Root(#[from] RootError),
    #[error(transparent)]
    StoreKino(#[from] StoreKinoError),
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Assign(#[from] AssignError),
    #[error(transparent)]
    Kinograph(#[from] KinographError),
    #[error(transparent)]
    Archive(#[from] ArchiveError),
    #[error("invalid hash `{value}`: {err}")]
    InvalidHash {
        value: String,
        #[source]
        err: HashParseError,
    },
    #[error("identity {id} has {} heads at commit time: {}", .heads.len(), .heads.join(", "))]
    MultipleHeads { id: String, heads: Vec<String> },
    #[error("identity {id} has no head (event graph cycle?)")]
    NoHead { id: String },
    #[error("prior root pointer references version {version} but no matching event is in the ledger")]
    PriorEventMissing { version: String },
    #[error("root pointer at {} is not a 64-hex hash: {body:?}", .path.display())]
    InvalidPointer { path: PathBuf, body: String },
    #[error("invalid root name {name:?}: must be a single path component with no `/`, `\\`, or `..`")]
    InvalidRootName { name: String },
    /// A kino has two or more live (non-superseded) assign events pointing
    /// at it. The commit cannot decide ownership; the user must author a
    /// tie-breaking assign whose `supersedes` list names all candidates.
    #[error("ambiguous assigns for kino {kino_id}: {} live candidates", .candidates.len())]
    AmbiguousAssign { kino_id: String, candidates: Vec<AssignCandidate> },
    /// A live assign references a root name that is not declared in
    /// `config.styx`. Raised during `commit_root` regardless of which
    /// root is currently being committed — an undeclared target is a
    /// config/user error that must be fixed globally.
    #[error("unknown root `{name}` referenced by assign event {event_hash}")]
    UnknownRoot { name: String, event_hash: String },
}

/// Inputs for a commit call. Mirrors the parts of `StoreKinoParams` that
/// the root-kino event also needs.
#[derive(Debug, Clone)]
pub struct CommitParams {
    pub author: String,
    pub provenance: String,
    pub ts: String,
}

#[derive(Debug, Clone)]
pub struct CommitResult {
    pub root_name: String,
    /// Content hash of the newly stored root version. `None` iff the call
    /// was a no-op (either nothing to promote, or the new bytes matched the
    /// prior version byte-for-byte).
    pub new_version: Option<Hash>,
    pub prior_version: Option<Hash>,
    /// Referencing-root → number of entries in the committed root that
    /// survived policy-based GC solely because another root's composition
    /// kinograph pointed at them. Empty when no cross-root protection
    /// fired.
    pub retained_by_cross_root: BTreeMap<String, usize>,
}

/// Snapshot of cross-root composition references. Built once at the start
/// of a commit batch (or standalone commit) by walking every declared
/// root's last-committed root-kinograph and collecting the `(target_id,
/// target_version)` pointers that each composition kinograph entry names.
///
/// Used during GC / staged-prune: any entry whose `(id, version)` appears as
/// a target is treated as implicitly pinned — protected from drop even
/// when policy would otherwise evict it.
#[derive(Debug, Default, Clone)]
pub struct ExternalRefs {
    /// `(target_id, target_version)` → set of root names whose composition
    /// kinograph entries point at that target.
    by_target: BTreeMap<(String, String), BTreeSet<String>>,
}

impl ExternalRefs {
    /// Build a global snapshot by walking every declared root's latest
    /// root kinograph (if any), iterating `kind == "kinograph"` entries,
    /// fetching each kinograph blob, parsing it, and recording each
    /// composition entry's target along with the source root that
    /// referenced it.
    ///
    /// The snapshot does not filter by self — query methods take
    /// `self_root` and drop references whose source equals it. That lets
    /// `commit_all` compute this once per batch instead of once per
    /// root.
    ///
    /// Unpinned composition entries resolve to the referenced id's head
    /// version via `pick_head` against `events`. Resolution failures (no
    /// events, fork) are skipped — cross-root integrity is best-effort.
    ///
    /// Any root pointer that cannot be resolved (missing blob, parse
    /// failure) is silently skipped: that root's own per-root commit
    /// will surface the failure on its own, and cross-root integrity is
    /// a best-effort layer.
    #[fastrace::trace]
    pub fn collect(
        kinora_root: &Path,
        declared_roots: &BTreeSet<String>,
        events: &[Event],
    ) -> Result<Self, CommitError> {
        let store = ContentStore::new(kinora_root);
        let mut out: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();
        let mut events_by_id: BTreeMap<String, Vec<&Event>> = BTreeMap::new();
        for e in events {
            if !e.is_store_event() || e.kind == "root" {
                continue;
            }
            events_by_id.entry(e.id.clone()).or_default().push(e);
        }

        for source_root in declared_roots {
            let root_ptr = match read_root_pointer(kinora_root, source_root) {
                Ok(Some(h)) => h,
                Ok(None) => continue,
                Err(e) => {
                    log::debug!(
                        target: "kinora::commit::refs",
                        source_root = source_root.as_str(),
                        error:% = e;
                        "skip root: unreadable pointer",
                    );
                    continue;
                }
            };
            let root_bytes = match store.read(&root_ptr) {
                Ok(b) => b,
                Err(e) => {
                    log::debug!(
                        target: "kinora::commit::refs",
                        source_root = source_root.as_str(),
                        root_hash = root_ptr.as_hex(),
                        error:% = e;
                        "skip root: missing root blob",
                    );
                    continue;
                }
            };
            let root_kg = match RootKinograph::parse(&root_bytes) {
                Ok(k) => k,
                Err(e) => {
                    log::debug!(
                        target: "kinora::commit::refs",
                        source_root = source_root.as_str(),
                        root_hash = root_ptr.as_hex(),
                        error:% = e;
                        "skip root: root blob did not parse",
                    );
                    continue;
                }
            };
            for entry in &root_kg.entries {
                if entry.kind != "kinograph" {
                    continue;
                }
                let version_hash = match Hash::from_str(&entry.version) {
                    Ok(h) => h,
                    Err(e) => {
                        log::debug!(
                            target: "kinora::commit::refs",
                            source_root = source_root.as_str(),
                            version = entry.version.as_str(),
                            error:% = e;
                            "skip entry: invalid version hash",
                        );
                        continue;
                    }
                };
                let kg_bytes = match store.read(&version_hash) {
                    Ok(b) => b,
                    Err(e) => {
                        log::debug!(
                            target: "kinora::commit::refs",
                            source_root = source_root.as_str(),
                            version = entry.version.as_str(),
                            error:% = e;
                            "skip entry: missing kinograph blob",
                        );
                        continue;
                    }
                };
                let kg = match Kinograph::parse(&kg_bytes) {
                    Ok(k) => k,
                    Err(e) => {
                        log::debug!(
                            target: "kinora::commit::refs",
                            source_root = source_root.as_str(),
                            version = entry.version.as_str(),
                            error:% = e;
                            "skip entry: kinograph did not parse",
                        );
                        continue;
                    }
                };
                for comp in &kg.entries {
                    let target_version = if !comp.pin.is_empty() {
                        comp.pin.clone()
                    } else {
                        let Some(group) = events_by_id.get(&comp.id) else {
                            continue;
                        };
                        match pick_head(&comp.id, group) {
                            Ok(head) => head.hash.clone(),
                            Err(e) => {
                                log::debug!(
                                    target: "kinora::commit::refs",
                                    source_root = source_root.as_str(),
                                    kino_id = comp.id.as_str(),
                                    error:% = e;
                                    "skip comp: head pick failed",
                                );
                                continue;
                            }
                        }
                    };
                    out.entry((comp.id.clone(), target_version))
                        .or_default()
                        .insert(source_root.clone());
                }
            }
        }
        Ok(Self { by_target: out })
    }

    /// `(id, version)` pairs referenced by any root other than
    /// `self_root`. Self-references don't need cross-root protection —
    /// explicit pinning already covers that case.
    ///
    /// Keyed on the pair (not just version) so two kinos that happen to
    /// share an identical content hash can't cross-contaminate each
    /// other's protection: only the exact `(id, version)` combo a
    /// referencing kinograph names is implicitly pinned.
    fn implicit_pinned_versions(&self, self_root: &str) -> BTreeSet<(String, String)> {
        self.by_target
            .iter()
            .filter(|(_, sources)| sources.iter().any(|s| s != self_root))
            .map(|((id, v), _)| (id.clone(), v.clone()))
            .collect()
    }

    /// Referencing-root names for a given `(id, version)` pair, excluding
    /// `self_root`. Returns `None` when no external root references it.
    fn referencing_roots(
        &self,
        id: &str,
        version: &str,
        self_root: &str,
    ) -> Option<BTreeSet<String>> {
        let set = self.by_target.get(&(id.to_owned(), version.to_owned()))?;
        let filtered: BTreeSet<String> = set
            .iter()
            .filter(|s| s.as_str() != self_root)
            .cloned()
            .collect();
        if filtered.is_empty() {
            None
        } else {
            Some(filtered)
        }
    }
}

/// Validate that `root_name` is a single safe path component. Rejects
/// empty strings, names containing `/` or `\`, and `..` / `.`. The pointer
/// file lives at `.kinora/roots/<name>`, so a name with traversal pieces
/// could escape the dir — block it defensively even though the CLI layer
/// ought to hand us well-formed input.
pub fn validate_root_name(name: &str) -> Result<(), CommitError> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
    {
        return Err(CommitError::InvalidRootName { name: name.to_owned() });
    }
    Ok(())
}

/// Read the current root pointer file. Returns `None` when the file does
/// not yet exist (no commit has happened for this root). The body is
/// expected to be exactly a 64-hex hash with no trailing whitespace.
pub fn read_root_pointer(
    kinora_root: &Path,
    root_name: &str,
) -> Result<Option<Hash>, CommitError> {
    validate_root_name(root_name)?;
    let path = root_pointer_path(kinora_root, root_name);
    match fs::read_to_string(&path) {
        Ok(body) => {
            let trimmed = body.trim_end_matches(['\r', '\n']);
            let hash = Hash::from_str(trimmed).map_err(|_| CommitError::InvalidPointer {
                path: path.clone(),
                body,
            })?;
            Ok(Some(hash))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(CommitError::Io(e)),
    }
}

/// Atomically write `.kinora/roots/<name>` with the given 64-hex hash
/// (no trailing newline). Uses tmp+rename so a crash mid-write never
/// leaves a truncated pointer.
fn write_root_pointer(
    kinora_root: &Path,
    root_name: &str,
    hash: &Hash,
) -> Result<(), CommitError> {
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
/// regardless of which root is being committed — an undeclared target is a
/// global config/user problem that needs fixing before any root is clean.
///
/// Events of kind `root` are skipped: a root kinograph represents the state
/// of user content, not its own history.
pub fn build_root(
    events: &[Event],
    root_name: &str,
    declared_roots: &BTreeSet<String>,
    prior_root: Option<&RootKinograph>,
) -> Result<RootKinograph, CommitError> {
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
            head.ts.clone(),
        ));
    }

    // Merge in prior-root entries that are missing from the fresh build.
    // Needed for Never-policy roots: their staged events get pruned after
    // each commit, so a rebuild with no new activity would otherwise lose
    // every prior entry. Respect live reassigns — if a live assign moves
    // the kino to a different root, the prior entry is *not* resurrected
    // under this one. Absence of any live assign means the original assign
    // was pruned; keep the entry.
    if let Some(prior) = prior_root {
        let existing: BTreeSet<String> =
            entries.iter().map(|e| e.id.clone()).collect();
        for entry in &prior.entries {
            if existing.contains(&entry.id) {
                continue;
            }
            let target = kino_target_root(&entry.id, &live_assigns, declared_roots)?;
            if target.as_deref().is_some_and(|t| t != root_name) {
                continue;
            }
            entries.push(entry.clone());
        }
        entries.sort_by(|a, b| a.id.cmp(&b.id));
    }

    Ok(RootKinograph { entries })
}

/// Collect the live assign set from the staged event stream.
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
) -> Result<Vec<(Hash, AssignEvent)>, CommitError> {
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
) -> Result<Option<String>, CommitError> {
    let mine: Vec<&(Hash, AssignEvent)> = live_assigns
        .iter()
        .filter(|(_, a)| a.kino_id == kino_id)
        .collect();
    match mine.len() {
        0 => Ok(None),
        1 => {
            let (h, a) = mine[0];
            if !declared_roots.contains(&a.target_root) {
                return Err(CommitError::UnknownRoot {
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
            Err(CommitError::AmbiguousAssign {
                kino_id: kino_id.to_owned(),
                candidates,
            })
        }
    }
}

fn pick_head<'a>(id: &str, events: &[&'a Event]) -> Result<&'a Event, CommitError> {
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
        [] => Err(CommitError::NoHead { id: id.to_owned() }),
        many => Err(CommitError::MultipleHeads {
            id: id.to_owned(),
            heads: many.iter().map(|e| e.hash.clone()).collect(),
        }),
    }
}

/// Run a commit pass for the named root.
///
/// Genesis (no prior pointer): stores the new root as a birth event (`id`
/// auto-set to the blob hash, empty `parents`).
/// Subsequent: stores the new root as a version event whose `id` matches the
/// prior root's id and `parents` lists the prior version hash.
///
/// No-op: returns `new_version = None` when either
///  - no prior pointer exists AND there are no staged events to promote, or
///  - a prior pointer exists AND the fresh canonical bytes match it.
#[fastrace::trace]
pub fn commit_root(
    kinora_root: &Path,
    root_name: &str,
    params: CommitParams,
) -> Result<CommitResult, CommitError> {
    validate_root_name(root_name)?;
    let cfg_path = config_path(kinora_root);
    let cfg_text = fs::read_to_string(&cfg_path)?;
    let config = Config::from_styx(&cfg_text)?;
    let declared_roots: BTreeSet<String> = config.roots.keys().cloned().collect();

    let ledger = Ledger::new(kinora_root);
    let events = ledger.read_all_events()?;

    let refs = ExternalRefs::collect(kinora_root, &declared_roots, &events)?;
    commit_root_with_refs(
        kinora_root,
        root_name,
        params,
        &config,
        &declared_roots,
        &events,
        &refs,
    )
}

/// Inner commit helper that takes a precomputed `ExternalRefs` snapshot
/// (built once per `commit_all` batch) and skips the config + event
/// re-reads. Standalone `commit_root` calls wrap this with a per-call
/// snapshot.
#[fastrace::trace]
#[allow(clippy::too_many_arguments)]
fn commit_root_with_refs(
    kinora_root: &Path,
    root_name: &str,
    params: CommitParams,
    config: &Config,
    declared_roots: &BTreeSet<String>,
    events: &[Event],
    refs: &ExternalRefs,
) -> Result<CommitResult, CommitError> {
    validate_root_name(root_name)?;
    let prior_version = read_root_pointer(kinora_root, root_name)?;

    let policy = config
        .roots
        .get(root_name)
        .cloned()
        .unwrap_or(RootPolicy::Never);

    // Load prior root kinograph (if any) so we can carry pinned entries
    // forward. Pinned entries survive rebuilds verbatim for kinos still
    // owned by this root — this preserves hand-edits to the pin/version
    // fields across commits. Loaded before `build_root` so the merge
    // step can pick up entries whose store events have been pruned.
    let prior_root: Option<RootKinograph> = match &prior_version {
        Some(h) => Some(RootKinograph::parse(
            &ContentStore::new(kinora_root).read(h)?,
        )?),
        None => None,
    };

    // Never and MaxAge roots get prior-root merge in `build_root`. After
    // archival, Never/MaxAge drain their committed events from staging —
    // so entries surviving across commits must come from prior_root.
    // MaxAge additionally relies on `apply_root_entry_gc` (driven by
    // `head_ts` on the entry) to age entries out of the merged result.
    // KeepLastN retains its entries via staging instead, so feeding
    // prior_root would spuriously resurrect superseded versions there.
    let merge_source = matches!(policy, RootPolicy::Never | RootPolicy::MaxAge(_))
        .then_some(prior_root.as_ref())
        .flatten();
    let mut root = build_root(events, root_name, declared_roots, merge_source)?;
    propagate_pins(&mut root, prior_root.as_ref());

    let implicit_pinned = refs.implicit_pinned_versions(root_name);

    // Policy-driven root-entry GC. MaxAge drops unpinned entries whose head
    // ts is older than the cutoff; Never and KeepLastN leave entries alone.
    // Entries whose version is implicitly pinned by a cross-root reference
    // are also protected — the retention-by-cross-root report records who
    // saved each one.
    let retained_by_cross_root = apply_root_entry_gc(
        &mut root,
        root_name,
        &policy,
        &params.ts,
        &implicit_pinned,
        refs,
    )?;

    let new_bytes = root.to_styxl()?.into_bytes();

    let result = match &prior_version {
        Some(prior) => {
            let prior_bytes = ContentStore::new(kinora_root).read(prior)?;
            if prior_bytes == new_bytes {
                CommitResult {
                    root_name: root_name.to_owned(),
                    new_version: None,
                    prior_version: prior_version.clone(),
                    retained_by_cross_root: retained_by_cross_root.clone(),
                }
            } else {
                let prior_event = events
                    .iter()
                    .find(|e| e.hash == prior.as_hex())
                    .ok_or_else(|| CommitError::PriorEventMissing {
                        version: prior.as_hex().to_owned(),
                    })?;
                let stored = store_kino(
                    kinora_root,
                    StoreKinoParams {
                        kind: "root".into(),
                        content: new_bytes,
                        author: params.author.clone(),
                        provenance: params.provenance.clone(),
                        ts: params.ts.clone(),
                        metadata: BTreeMap::new(),
                        id: Some(prior_event.id.clone()),
                        parents: vec![prior.as_hex().to_owned()],
                    },
                )?;
                let new_hash = Hash::from_str(&stored.event.hash).map_err(|err| {
                    CommitError::InvalidHash {
                        value: stored.event.hash.clone(),
                        err,
                    }
                })?;
                write_root_pointer(kinora_root, root_name, &new_hash)?;
                CommitResult {
                    root_name: root_name.to_owned(),
                    new_version: Some(new_hash),
                    prior_version: prior_version.clone(),
                    retained_by_cross_root: retained_by_cross_root.clone(),
                }
            }
        }
        None => {
            if root.entries.is_empty() {
                CommitResult {
                    root_name: root_name.to_owned(),
                    new_version: None,
                    prior_version: None,
                    retained_by_cross_root: retained_by_cross_root.clone(),
                }
            } else {
                let stored = store_kino(
                    kinora_root,
                    StoreKinoParams {
                        kind: "root".into(),
                        content: new_bytes,
                        author: params.author.clone(),
                        provenance: params.provenance.clone(),
                        ts: params.ts.clone(),
                        metadata: BTreeMap::new(),
                        id: None,
                        parents: vec![],
                    },
                )?;
                let new_hash = Hash::from_str(&stored.event.hash).map_err(|err| {
                    CommitError::InvalidHash {
                        value: stored.event.hash.clone(),
                        err,
                    }
                })?;
                write_root_pointer(kinora_root, root_name, &new_hash)?;
                CommitResult {
                    root_name: root_name.to_owned(),
                    new_version: Some(new_hash),
                    prior_version: None,
                    retained_by_cross_root: retained_by_cross_root.clone(),
                }
            }
        }
    };

    // Non-commits roots that actually promoted work emit a per-commit
    // archive kino so staging can be cleaned without losing provenance.
    // The archive is content-addressed; re-running commit with no new
    // activity is idempotent (same owned events → same archive hash →
    // same assign). commits root itself is excluded (its kinograph already
    // lists each archive; an archive-of-archives would just duplicate
    // that record).
    //
    // Never and MaxAge roots additionally drop the just-archived events
    // from staging — provenance is preserved in the archive kino, so
    // keeping them would only bloat the ledger. MaxAge retention lives
    // on the root kinograph (via `apply_root_entry_gc` + `head_ts`),
    // so staging no longer carries the retention signal.
    if root_name != COMMITS_ROOT
        && result.new_version.is_some()
        && let Some((_archive_id, archived_hashes)) = maybe_archive_owned_events(
            kinora_root,
            root_name,
            events,
            declared_roots,
            &params,
        )?
        && matches!(policy, RootPolicy::Never | RootPolicy::MaxAge(_))
    {
        drop_staged_events(kinora_root, &archived_hashes)?;
    }

    // commits root under Never/MaxAge: drop its owned staged events
    // (archive store events + the archive-assigns that routed them here).
    // Without this, every per-root commit would leak one archive + assign
    // pair into staging forever, even though the commits kinograph
    // already records them. Must drop ALL owned events (not just
    // archive-assigns): otherwise an orphaned archive store event would
    // default-route to inbox on a later commit, leaking the archive into
    // inbox's view. KeepLastN on commits would still drain here since
    // archives must never stay in staging beyond the current commit.
    if root_name == COMMITS_ROOT
        && result.new_version.is_some()
        && matches!(policy, RootPolicy::Never | RootPolicy::MaxAge(_))
    {
        let owned = collect_owned_staged_events(events, root_name, declared_roots)?;
        let owned_hashes: Vec<Hash> = owned
            .iter()
            .map(|e| e.event_hash())
            .collect::<Result<_, _>>()?;
        drop_staged_events(kinora_root, &owned_hashes)?;
    }

    // Prune staged events owned by this root per policy. Runs after the pointer
    // write so a crash mid-prune leaves the ledger larger than strictly
    // required (safe) rather than smaller than the pointer can resolve.
    prune_staged_events(
        kinora_root,
        events,
        root_name,
        declared_roots,
        &root,
        &policy,
        &implicit_pinned,
    )?;

    Ok(result)
}

/// Collect all staged events owned by `root_name` — store events routed
/// via live assigns (default inbox), and assign events whose target is
/// this root. Store events of kind `root` are excluded (they form the
/// commit parent chain and are not user content).
fn collect_owned_staged_events<'e>(
    events: &'e [Event],
    root_name: &str,
    declared_roots: &BTreeSet<String>,
) -> Result<Vec<&'e Event>, CommitError> {
    let live_assigns = collect_live_assigns(events)?;
    let mut owned: Vec<&Event> = Vec::new();
    for e in events {
        if e.event_kind == EVENT_KIND_ASSIGN {
            // `collect_live_assigns` above already parsed every assign,
            // so any malformed one would have surfaced there — match its
            // `?` style instead of silently dropping.
            let a = AssignEvent::from_event(e)?;
            if a.target_root == root_name {
                owned.push(e);
            }
            continue;
        }
        if !e.is_store_event() || e.kind == "root" {
            continue;
        }
        let target = kino_target_root(&e.id, &live_assigns, declared_roots)?;
        let target_name = target.as_deref().unwrap_or("inbox");
        if target_name == root_name {
            owned.push(e);
        }
    }
    Ok(owned)
}

/// Archive the staged events owned by `root_name` into a `commit-archive`
/// kino, and produce an assign targeting `commits` so the commits root
/// can incorporate it on its own commit step.
///
/// Returns `Ok(None)` when the root has no owned events (nothing to
/// archive — commit produced no new version either, in practice). Returns
/// `Ok(Some((archive_id, archived_hashes)))` otherwise — the caller uses
/// the hashes to prune the archived events from staging when the root's
/// policy is `Never`. Idempotent: re-running with the same owned-event
/// set produces the same archive kino id (content-addressed) and skips
/// writing a duplicate assign.
fn maybe_archive_owned_events(
    kinora_root: &Path,
    root_name: &str,
    events: &[Event],
    declared_roots: &BTreeSet<String>,
    params: &CommitParams,
) -> Result<Option<(String, Vec<Hash>)>, CommitError> {
    // The caller only invokes us after a real version bump, so in the
    // happy path `owned_refs` is always non-empty. This guard is a
    // crash-recovery safety net: if a prior run wrote the pointer but
    // died before staging the archive, a replay against an already-clean
    // ledger (staged events consumed by GC) would find nothing to archive
    // and must bail cleanly rather than write a zero-event archive.
    let owned_refs = collect_owned_staged_events(events, root_name, declared_roots)?;
    if owned_refs.is_empty() {
        return Ok(None);
    }
    let owned_hashes: Vec<Hash> = owned_refs
        .iter()
        .map(|e| e.event_hash())
        .collect::<Result<_, _>>()?;
    let owned: Vec<Event> = owned_refs.into_iter().cloned().collect();
    let archive_bytes = serialize_archive(&owned)?;

    let mut metadata = BTreeMap::new();
    metadata.insert("name".into(), format!("{root_name}-commit-archive"));

    let stored = store_kino(
        kinora_root,
        StoreKinoParams {
            kind: ARCHIVE_CONTENT_KIND.into(),
            content: archive_bytes,
            author: params.author.clone(),
            provenance: params.provenance.clone(),
            ts: params.ts.clone(),
            metadata,
            id: None,
            parents: vec![],
        },
    )?;
    let archive_id = stored.event.id.clone();

    // Skip writing a new assign if a live one already targets commits for
    // this archive kino. Same crash-recovery safety net as above: in the
    // happy path the archive kino we just stored is new, so no live assign
    // can exist yet. If a prior run wrote the archive store but crashed
    // before the assign, this guard lets the replay pick up where it left
    // off without duplicating the assign (which would trip AmbiguousAssign).
    let already_assigned = events.iter().any(|e| {
        if e.event_kind != EVENT_KIND_ASSIGN {
            return false;
        }
        let Ok(a) = AssignEvent::from_event(e) else {
            return false;
        };
        a.kino_id == archive_id && a.target_root == COMMITS_ROOT
    });
    if !already_assigned {
        let assign = AssignEvent {
            kino_id: archive_id.clone(),
            target_root: COMMITS_ROOT.into(),
            supersedes: vec![],
            author: params.author.clone(),
            ts: params.ts.clone(),
            provenance: params.provenance.clone(),
        };
        write_assign(kinora_root, &assign)?;
    }
    Ok(Some((archive_id, owned_hashes)))
}

/// Remove the named events from the staged ledger. Tolerates `NotFound`
/// (idempotent under crash recovery). Called from the Never-policy
/// post-archive prune path in `commit_root_with_refs`.
fn drop_staged_events(
    kinora_root: &Path,
    hashes: &[Hash],
) -> Result<(), CommitError> {
    for h in hashes {
        let path = staged_event_path(kinora_root, h);
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(CommitError::Io(e)),
        }
    }
    Ok(())
}

/// Copy `pin` + `version` from prior root's entries into the freshly built
/// root, for kinos still owned by this root. Unpinned prior entries are
/// ignored — the rebuilt head already represents them.
///
/// Pin wins over head: if the rebuild would otherwise bump an entry to a
/// newer head, pin propagation overrides `version` back to the pinned
/// snapshot. This is the whole point of pinning — the user froze that
/// specific version and new stores for the same kino id should not
/// shadow it until the pin is released.
///
/// Ownership wins over pin on cross-root moves: a kino reassigned away
/// from this root simply won't appear in the rebuild (routing filtered
/// it out), so its prior pinned entry is silently dropped.
fn propagate_pins(root: &mut RootKinograph, prior: Option<&RootKinograph>) {
    let Some(prior) = prior else { return };
    let pinned: BTreeMap<&str, &RootEntry> = prior
        .entries
        .iter()
        .filter(|e| e.pin)
        .map(|e| (e.id.as_str(), e))
        .collect();
    for entry in root.entries.iter_mut() {
        if let Some(prior_entry) = pinned.get(entry.id.as_str()) {
            entry.pin = true;
            entry.version = prior_entry.version.clone();
            // head_ts must track the version it names — otherwise a pinned
            // entry would report the *fresh* head's ts while pointing at
            // the prior version's hash.
            entry.head_ts = prior_entry.head_ts.clone();
        }
    }
}

/// Drop root entries whose head ts is older than the `MaxAge` cutoff,
/// unless the entry is pinned (explicitly or implicitly by a cross-root
/// reference). No-op for `Never` and `KeepLastN` — `KeepLastN` acts on
/// the staged ledger, not the root view (the root view already has at most
/// one entry per kino by `pick_head`).
///
/// Returns a `referencing-root → count` map listing how many entries were
/// rescued from GC by an external reference. This report feeds into the
/// CLI's retention hint.
fn apply_root_entry_gc(
    root: &mut RootKinograph,
    root_name: &str,
    policy: &RootPolicy,
    now: &str,
    implicit_pinned: &BTreeSet<(String, String)>,
    refs: &ExternalRefs,
) -> Result<BTreeMap<String, usize>, CommitError> {
    let mut retained: BTreeMap<String, usize> = BTreeMap::new();
    let Some(max_age_s) = policy.max_age_seconds() else {
        return Ok(retained);
    };
    let now_ts = parse_ts(now)?;
    let cutoff = now_ts - max_age_s;
    let mut implicit_hits: Vec<(String, String)> = Vec::new();
    root.entries.retain(|entry| {
        if entry.pin {
            return true;
        }
        if entry.head_ts.is_empty() {
            // Legacy entry (pre-0sgr kinograph) — no ts signal to compare
            // against. Stay conservative and keep rather than evict on
            // unverifiable state.
            return true;
        }
        let old = match parse_ts(&entry.head_ts) {
            Ok(t) => t < cutoff,
            Err(e) => {
                log::warn!(
                    target: "kinora::commit::gc",
                    root = root_name,
                    kino_id = entry.id.as_str(),
                    version = entry.version.as_str(),
                    ts = entry.head_ts.as_str(),
                    error:% = e;
                    "unparseable head_ts on entry; keeping entry",
                );
                false
            }
        };
        if !old {
            return true;
        }
        let key = (entry.id.clone(), entry.version.clone());
        if implicit_pinned.contains(&key) {
            implicit_hits.push(key);
            return true;
        }
        false
    });
    for (id, version) in implicit_hits {
        if let Some(referencing) = refs.referencing_roots(&id, &version, root_name) {
            for r in referencing {
                *retained.entry(r.clone()).or_insert(0) += 1;
            }
        }
    }
    Ok(retained)
}

/// Parse an RFC3339 timestamp into seconds-since-epoch. Errors are
/// surfaced as `CommitError::Event` via `EventError::Parse`.
fn parse_ts(s: &str) -> Result<i64, CommitError> {
    use std::str::FromStr;
    jiff::Timestamp::from_str(s)
        .map(|t| t.as_second())
        .map_err(|e| {
            CommitError::Event(EventError::Parse(format!(
                "invalid ts {s:?}: {e}"
            )))
        })
}

/// Prune staged events owned by `root_name` per `policy`.
///
/// Ownership: a store event belongs to the root that its kino id routes
/// to (via the live-assign graph, default inbox). An assign event belongs
/// to `target_root` even if it is superseded — so old assigns age out
/// alongside the store events they used to route.
///
/// Protections: (1) pinned versions named by root entries never drop, and
/// neither does the staged event whose hash equals that version. (2) root-kind
/// events never drop — they form the commit parent chain. (3) store
/// events whose hash appears in a live-assign's target kino graph: we keep
/// the pick_head event (the root's current view) implicitly because
/// entries point at it and the content store still holds the blob; but on
/// KeepLastN specifically we protect the head event from the N-window by
/// always including it in the survivor set.
#[allow(clippy::too_many_arguments)]
fn prune_staged_events(
    kinora_root: &Path,
    events: &[Event],
    root_name: &str,
    declared_roots: &BTreeSet<String>,
    root: &RootKinograph,
    policy: &RootPolicy,
    implicit_pinned: &BTreeSet<(String, String)>,
) -> Result<(), CommitError> {
    // MaxAge retention now lives on the root kinograph (via
    // `apply_root_entry_gc` + `head_ts`). Never+MaxAge drain archived
    // events from staging post-archive, so staging-side pruning is both
    // unnecessary and incorrect (it would prune events that haven't been
    // archived yet). Only KeepLastN still runs here — it keeps N versions
    // per kino in staging and so relies on staging retention.
    let RootPolicy::KeepLastN(n) = policy else {
        return Ok(());
    };

    // Explicitly pinned versions from this root's kinograph (content
    // hashes). Cross-root implicit pins are handled separately below
    // because they need `(id, version)` matching — two kinos with
    // colliding content hashes should not cross-contaminate protection.
    let pinned_versions: BTreeSet<&str> = root
        .entries
        .iter()
        .filter(|e| e.pin)
        .map(|e| e.version.as_str())
        .collect();

    // Implicit pin check: a store event is cross-root-protected only
    // when `(id, hash)` matches a ref target exactly.
    let is_implicit_pinned = |id: &str, version: &str| -> bool {
        implicit_pinned.contains(&(id.to_owned(), version.to_owned()))
    };

    // Recompute live-assign routing to find each store event's owning root.
    let live_assigns = collect_live_assigns(events)?;

    // Pre-index: for each kino id routed to this root, the set of its
    // store events (ordered by ts for KeepLastN).
    let mut owned_stores_by_id: BTreeMap<String, Vec<&Event>> = BTreeMap::new();
    for e in events {
        if e.event_kind == EVENT_KIND_ASSIGN {
            continue;
        }
        if !e.is_store_event() || e.kind == "root" {
            continue;
        }
        let target = kino_target_root(&e.id, &live_assigns, declared_roots)?;
        let target_name = target.as_deref().unwrap_or("inbox");
        if target_name == root_name {
            owned_stores_by_id.entry(e.id.clone()).or_default().push(e);
        }
    }

    let mut drop_hashes: BTreeSet<Hash> = BTreeSet::new();

    // Per kino id: sort store events by ts desc, keep the N newest; mark
    // the remainder for drop unless they are pinned versions. Assigns
    // are NOT touched by KeepLastN (policy acts on store versions only).
    for (_, mut group) in owned_stores_by_id {
        group.sort_by(|a, b| b.ts.cmp(&a.ts));
        for (idx, e) in group.iter().enumerate() {
            if idx < *n {
                continue;
            }
            if pinned_versions.contains(e.hash.as_str()) {
                continue;
            }
            if is_implicit_pinned(&e.id, &e.hash) {
                continue;
            }
            drop_hashes.insert(e.event_hash()?);
        }
    }

    for h in drop_hashes {
        let path = staged_event_path(kinora_root, &h);
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(CommitError::Io(e)),
        }
    }
    Ok(())
}

/// One entry in the batch report produced by `commit_all`.
///
/// The outer tuple pairs the root's declared name with the per-root
/// outcome. A per-root `Err` surfaces the specific failure (e.g. a fork
/// on one root) without aborting the batch.
pub type CommitAllEntry = (String, Result<CommitResult, CommitError>);

/// Commit every root declared in `config.styx`, in name order.
///
/// Reads the config once, then calls `commit_root` per declared root.
/// Per-root errors are collected into the returned `Vec` — they don't
/// short-circuit the batch, so clean roots still advance to disk even
/// when a sibling root is in a failing state (e.g. a fork).
///
/// The outer `Result::Err` is reserved for pre-iteration failures:
/// config file missing, unreadable, or unparseable.
#[fastrace::trace]
pub fn commit_all(
    kinora_root: &Path,
    params: CommitParams,
) -> Result<Vec<CommitAllEntry>, CommitError> {
    let cfg_path = config_path(kinora_root);
    let cfg_text = fs::read_to_string(&cfg_path)?;
    let config = Config::from_styx(&cfg_text)?;
    let declared_roots: BTreeSet<String> = config.roots.keys().cloned().collect();

    let ledger = Ledger::new(kinora_root);
    let events = ledger.read_all_events()?;

    // Snapshot the cross-root composition pointers as they stand at batch
    // start. Roots committed earlier in this loop may produce new versions,
    // but the snapshot-at-start is what every root in the batch evaluates
    // against — conservative and deterministic.
    let refs = ExternalRefs::collect(kinora_root, &declared_roots, &events)?;

    // Iterate non-commits roots first (in name order), then `commits`
    // last. The commits root's job is to consume the archive-assigns each
    // other root produces during this batch — so it must run after every
    // sibling has had a chance to stage its archive.
    let non_commits: Vec<String> = config
        .roots
        .keys()
        .filter(|n| n.as_str() != COMMITS_ROOT)
        .cloned()
        .collect();
    let has_commits = config.roots.contains_key(COMMITS_ROOT);

    let mut out: Vec<CommitAllEntry> = Vec::with_capacity(config.roots.len());
    for name in &non_commits {
        let result = commit_root_with_refs(
            kinora_root,
            name,
            params.clone(),
            &config,
            &declared_roots,
            &events,
            &refs,
        );
        out.push((name.clone(), result));
    }

    // Re-read the ledger before committing the commits root so it can see
    // every archive-assign that was just staged. The non-commits iterations
    // above may have appended archive store + assign events to staging; the
    // `events` snapshot from the top of this function predates them.
    if has_commits {
        let fresh_events = ledger.read_all_events()?;
        let fresh_refs = ExternalRefs::collect(kinora_root, &declared_roots, &fresh_events)?;
        let result = commit_root_with_refs(
            kinora_root,
            COMMITS_ROOT,
            params.clone(),
            &config,
            &declared_roots,
            &fresh_events,
            &fresh_refs,
        );
        out.push((COMMITS_ROOT.to_owned(), result));
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assign::{AssignEvent, EVENT_KIND_ASSIGN};
    use crate::commit_archive::parse_archive;
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

    fn params(author: &str, ts: &str) -> CommitParams {
        CommitParams {
            author: author.into(),
            provenance: "commit-test".into(),
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
                provenance: "commit-test".into(),
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
            commit_root(&root, "inbox", params("yj", "2026-04-19T10:00:02Z")).unwrap();
        let hash = result.new_version.expect("new version on genesis");
        assert!(result.prior_version.is_none());

        let events = Ledger::new(&root).read_all_events().unwrap();
        let root_event = events.iter().find(|e| e.kind == "root").unwrap();
        assert_eq!(root_event.hash, hash.as_hex());
        assert!(root_event.parents.is_empty(), "genesis has empty parents");
        assert_eq!(root_event.id, root_event.hash, "genesis id == hash");
    }

    #[test]
    fn subsequent_commit_links_parent_and_bumps_version() {
        let (_t, root) = setup();
        store_md(&root, b"v1", "doc", "2026-04-19T10:00:00Z");

        let first = commit_root(&root, "inbox", params("yj", "2026-04-19T10:00:01Z"))
            .unwrap();
        let prior = first.new_version.unwrap();

        // Add a second kino so the second root differs.
        store_md(&root, b"second", "other", "2026-04-19T10:00:02Z");

        let second = commit_root(&root, "inbox", params("yj", "2026-04-19T10:00:03Z"))
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
    fn commit_is_no_op_when_nothing_new() {
        let (_t, root) = setup();
        store_md(&root, b"one", "only", "2026-04-19T10:00:00Z");
        let first = commit_root(&root, "inbox", params("yj", "2026-04-19T10:00:01Z"))
            .unwrap();
        let first_version = first.new_version.unwrap();

        let pointer_before = fs::read(root_pointer_path(&root, "inbox")).unwrap();

        // No new user events; different ts on the commit itself.
        let second = commit_root(&root, "inbox", params("yj", "2026-04-19T10:05:00Z"))
            .unwrap();
        assert!(second.new_version.is_none(), "should be no-op");
        assert_eq!(second.prior_version.unwrap(), first_version);

        let pointer_after = fs::read(root_pointer_path(&root, "inbox")).unwrap();
        assert_eq!(pointer_before, pointer_after, "pointer unchanged on no-op");
    }

    #[test]
    fn commit_ignores_non_store_events() {
        // A hand-forged non-store event in the staged ledger must not appear
        // in the committed root. Commit only sees content-track events.
        let (_t, root) = setup();
        store_md(&root, b"real", "doc", "2026-04-19T10:00:00Z");

        // Forge an event with a future/unknown event_kind. It is neither a
        // store event (so it must not land as a RootEntry) nor an assign
        // (so it must not be interpreted as one either). Commit should
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

        let result = commit_root(&root, "inbox", params("yj", "2026-04-19T10:00:05Z"))
            .unwrap();
        let hash = result.new_version.expect("expected commit to succeed");
        let bytes = ContentStore::new(&root).read(&hash).unwrap();
        let kinograph = RootKinograph::parse(&bytes).unwrap();
        assert!(
            kinograph.entries.iter().all(|k| k.id != forged.id),
            "forged non-store event leaked into root kinograph"
        );
    }

    #[test]
    fn commit_with_no_events_and_no_prior_is_no_op() {
        let (_t, root) = setup();
        let result = commit_root(&root, "inbox", params("yj", "2026-04-19T10:00:00Z"))
            .unwrap();
        assert!(result.new_version.is_none());
        assert!(result.prior_version.is_none());
        assert!(!root_pointer_path(&root, "inbox").exists());
    }

    #[test]
    fn two_independent_commits_produce_byte_identical_root_blobs() {
        // Run the same logical commit in two fresh repos with different
        // commit author/ts/provenance — the root blob (content bytes) must
        // be byte-identical because it's derived purely from the user events.
        let mk = |root: &Path| {
            store_md(root, b"alpha", "a", "2026-04-19T10:00:00Z");
            store_md(root, b"beta", "b", "2026-04-19T10:00:01Z");
            store_md(root, b"gamma", "c", "2026-04-19T10:00:02Z");
        };

        let (_t1, root1) = setup();
        mk(&root1);
        let r1 =
            commit_root(&root1, "inbox", params("alice", "2026-04-19T10:00:03Z"))
                .unwrap()
                .new_version
                .unwrap();

        let (_t2, root2) = setup();
        mk(&root2);
        let r2 = commit_root(
            &root2,
            "inbox",
            CommitParams {
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

        let result = commit_root(&root, "inbox", params("yj", "2026-04-19T10:00:10Z"))
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
        let result = commit_root(&root, "inbox", params("yj", "2026-04-19T10:00:01Z"))
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
        // Store 3 kinos → commit (3 entries). Then update one to v2 and
        // commit again — root should still have 3 entries, with one
        // entry's `version` bumped to the v2 hash.
        let (_t, root) = setup();
        let a = store_md(&root, b"a", "a", "2026-04-19T10:00:00Z");
        let b = store_md(&root, b"b", "b", "2026-04-19T10:00:01Z");
        let c = store_md(&root, b"c", "c", "2026-04-19T10:00:02Z");

        let first = commit_root(&root, "inbox", params("yj", "2026-04-19T10:00:03Z"))
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
                provenance: "commit-test".into(),
                ts: "2026-04-19T10:00:10Z".into(),
                metadata: BTreeMap::from([("name".into(), "b".into())]),
                id: Some(b.id.clone()),
                parents: vec![b.hash.clone()],
            },
        )
        .unwrap();

        let second = commit_root(&root, "inbox", params("yj", "2026-04-19T10:00:11Z"))
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
        assert!(matches!(err, CommitError::InvalidPointer { .. }), "got: {err:?}");
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
                commit_root(&root, name, params("yj", "2026-04-19T10:00:00Z")).unwrap_err();
            assert!(
                matches!(err, CommitError::InvalidRootName { .. }),
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
        let err = build_root(&[a, b], "inbox", &declared, None).unwrap_err();
        assert!(matches!(err, CommitError::NoHead { .. }), "got: {err:?}");
    }

    #[test]
    fn fork_rejected_as_multiple_heads() {
        // Two sibling versions off the same parent → fork. Commit must
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
                    provenance: "commit-test".into(),
                    ts: ts.into(),
                    metadata: BTreeMap::from([("name".into(), "doc".into())]),
                    id: Some(birth.id.clone()),
                    parents: vec![birth.hash.clone()],
                },
            )
            .unwrap();
        }

        let err =
            commit_root(&root, "inbox", params("yj", "2026-04-19T10:00:10Z")).unwrap_err();
        assert!(matches!(err, CommitError::MultipleHeads { .. }), "got: {err:?}");
    }

    // ------------------------------------------------------------------
    // commit_all (batch driver)
    // ------------------------------------------------------------------

    fn write_config(kin_root: &Path, body: &str) {
        fs::write(config_path(kin_root), body).unwrap();
    }

    #[test]
    fn commit_all_iterates_every_declared_root_in_name_order() {
        let (_t, root) = setup();
        // init writes a config with just `inbox`. Overwrite with three roots
        // listed out of alphabetical order in the file — commit_all should
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

        let entries = commit_all(&root, params("yj", "2026-04-19T10:00:01Z")).unwrap();
        let names: Vec<_> = entries.iter().map(|(n, _)| n.clone()).collect();
        // Non-commits roots run in name order (auto-provisioned `inbox`
        // included), then `commits` iterates last so it can consume the
        // archive-assigns the other roots just produced.
        assert_eq!(names, vec!["alpha", "inbox", "main", "zeta", "commits"]);
        assert!(
            entries.iter().all(|(_, r)| r.is_ok()),
            "every root should have committed cleanly: {entries:?}"
        );
    }

    #[test]
    fn commit_all_iterates_commits_root_last() {
        // `commits` must run after all other roots regardless of its
        // alphabetical position, so it can consume the archive-assigns the
        // other roots produce during their commits.
        let (_t, root) = setup();
        write_config(
            &root,
            r#"repo-url "https://example.com/x.git"
roots {
  zulu { policy "never" }
  alpha { policy "never" }
}
"#,
        );

        let entries = commit_all(&root, params("yj", "2026-04-19T10:00:01Z")).unwrap();
        let names: Vec<_> = entries.iter().map(|(n, _)| n.clone()).collect();
        assert_eq!(
            names.last().map(|s| s.as_str()),
            Some("commits"),
            "commits must run last; got: {names:?}",
        );
    }

    #[test]
    fn commit_all_per_root_errors_do_not_short_circuit_clean_roots() {
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
        // content store — commit_root will fail to read the prior blob
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

        let entries = commit_all(&root, params("yj", "2026-04-19T10:00:03Z")).unwrap();
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
    fn commit_all_surfaces_config_errors_as_outer_err() {
        let (_t, root) = setup();
        // Overwrite with an unparseable config.
        write_config(&root, "this is not valid styx {{{");
        let err = commit_all(&root, params("yj", "2026-04-19T10:00:00Z")).unwrap_err();
        assert!(
            matches!(err, CommitError::Config(_)),
            "config parse failure should be outer Err: {err:?}"
        );
    }

    #[test]
    fn commit_all_surfaces_missing_config_as_outer_err() {
        let (_t, root) = setup();
        fs::remove_file(config_path(&root)).unwrap();
        let err = commit_all(&root, params("yj", "2026-04-19T10:00:00Z")).unwrap_err();
        assert!(
            matches!(err, CommitError::Io(_)),
            "missing config.styx should be outer Err: {err:?}"
        );
    }

    #[test]
    fn commit_all_emits_no_op_entry_when_root_has_nothing_to_promote() {
        // Default init config has `inbox` and `commits` (both auto-provisioned).
        // No staged events → commit_all should still visit each and emit no-op
        // entries.
        let (_t, root) = setup();
        let entries = commit_all(&root, params("yj", "2026-04-19T10:00:00Z")).unwrap();
        assert_eq!(entries.len(), 2);
        let names: Vec<_> = entries.iter().map(|(n, _)| n.clone()).collect();
        // `commits` always runs last so it can sweep up archive-assigns.
        assert_eq!(names, vec!["inbox", "commits"]);
        for (_name, result) in &entries {
            let res = result.as_ref().unwrap();
            assert!(res.new_version.is_none());
            assert!(res.prior_version.is_none());
        }
    }

    // ------------------------------------------------------------------
    // 7mou: commit consumes assigns + AmbiguousAssign + UnknownRoot
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
            provenance: "commit-test".into(),
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
            commit_root(&root, "rfcs", params("yj", "2026-04-19T10:00:02Z")).unwrap();
        let rfcs_ids = root_ids(&root, &rfcs_res.new_version.unwrap());
        assert_eq!(rfcs_ids, vec![k.id.clone()], "rfcs should own the kino");

        let inbox_res =
            commit_root(&root, "inbox", params("yj", "2026-04-19T10:00:03Z")).unwrap();
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
            commit_root(&root, "inbox", params("yj", "2026-04-19T10:00:01Z")).unwrap();
        let inbox_ids = root_ids(&root, &inbox_res.new_version.unwrap());
        assert_eq!(inbox_ids, vec![k.id.clone()], "unassigned kino should land in inbox");

        let main_res =
            commit_root(&root, "main", params("yj", "2026-04-19T10:00:02Z")).unwrap();
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
            commit_root(&root, "designs", params("yj", "2026-04-19T10:00:03Z")).unwrap();
        let designs_ids = root_ids(&root, &designs_res.new_version.unwrap());
        assert_eq!(designs_ids, vec![k.id.clone()], "designs wins after supersede");

        let rfcs_res =
            commit_root(&root, "rfcs", params("yj", "2026-04-19T10:00:04Z")).unwrap();
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

        let c_res = commit_root(&root, "c", params("yj", "2026-04-19T10:00:04Z")).unwrap();
        let c_ids = root_ids(&root, &c_res.new_version.unwrap());
        assert_eq!(c_ids, vec![k.id.clone()]);

        let a_res = commit_root(&root, "a", params("yj", "2026-04-19T10:00:05Z")).unwrap();
        assert!(a_res.new_version.is_none(), "a's assign was superseded");

        let b_res = commit_root(&root, "b", params("yj", "2026-04-19T10:00:06Z")).unwrap();
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
            commit_root(&root, "rfcs", params("yj", "2026-04-19T10:00:03Z")).unwrap_err();
        match err {
            CommitError::AmbiguousAssign { kino_id, candidates } => {
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
            commit_root(&root, "inbox", params("yj", "2026-04-19T10:00:02Z")).unwrap_err();
        match err {
            CommitError::UnknownRoot { name, event_hash } => {
                assert_eq!(name, "madeup");
                assert_eq!(event_hash, h.as_hex());
            }
            other => panic!("expected UnknownRoot, got {other:?}"),
        }
    }

    #[test]
    fn cross_root_removal_kino_moves_from_main_to_rfcs() {
        let (_t, root) = setup();
        // keep-last-10 instead of Never: under kinora-bayr Never prunes
        // after archive, which would drop the kino's store event before
        // the reassign happens. This test is about cross-root routing,
        // not prune semantics, so use a policy that holds the ledger.
        write_config(
            &root,
            r#"repo-url "https://example.com/x.git"
roots {
  main { policy "keep-last-10" }
  rfcs { policy "keep-last-10" }
}
"#,
        );

        // Pin the kino to `main` first.
        let k = store_md(&root, b"doc", "doc", "2026-04-19T10:00:00Z");
        write_assign_for(&root, &k.id, "main", vec![], "2026-04-19T10:00:01Z");

        let first = commit_all(&root, params("yj", "2026-04-19T10:00:02Z")).unwrap();
        let by_name: std::collections::HashMap<_, _> =
            first.into_iter().collect();
        let main_v1 = by_name["main"].as_ref().unwrap().new_version.clone().unwrap();
        let main_ids = root_ids(&root, &main_v1);
        assert_eq!(main_ids, vec![k.id.clone()], "main should initially own the kino");

        // Reassign to rfcs; main's last-assign hash is looked up from the
        // previous step's event stream via the ledger. The ledger may also
        // contain archive-assigns pointing at `commits` (produced by the
        // archive lifecycle), so filter by kino id + target root to find the
        // specific assign we mean to supersede.
        let prior_assigns = Ledger::new(&root).read_all_events().unwrap();
        let prior_main_assign = prior_assigns
            .iter()
            .find(|e| {
                if e.event_kind != EVENT_KIND_ASSIGN {
                    return false;
                }
                let Ok(a) = AssignEvent::from_event(e) else {
                    return false;
                };
                a.kino_id == k.id && a.target_root == "main"
            })
            .unwrap();
        let supersedes_hash = prior_main_assign.event_hash().unwrap();
        write_assign_for(
            &root,
            &k.id,
            "rfcs",
            vec![supersedes_hash.as_hex().to_owned()],
            "2026-04-19T10:00:03Z",
        );

        // Re-commit both roots. Main should drop the kino; rfcs should gain it.
        let second = commit_all(&root, params("yj", "2026-04-19T10:00:04Z")).unwrap();
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
        // CommitError payload carries them through.
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
            commit_root(&root, "rfcs", params("yj", "2026-04-19T10:00:03Z")).unwrap_err();
        let candidates = match err {
            CommitError::AmbiguousAssign { candidates, .. } => candidates,
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

    use crate::paths::{staged_dir, staged_event_path};

    /// Replace the prior root pointer with a new root kinograph that pins
    /// the entry for `kino_id` to the given `version` hash. Writes the new
    /// blob to the content store, authors a root store event linked to the
    /// prior version via `parents`, and points `.kinora/roots/<name>` at
    /// the new version. Mirrors what a user hand-editing the root to add a
    /// pin would produce, so subsequent `commit_root` calls see the pin.
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

    /// Count staged event files on disk.
    fn staged_event_count(kin: &Path) -> usize {
        let dir = staged_dir(kin);
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

    /// True iff the staged event file for the given event exists.
    ///
    /// Staged events are keyed by `event.event_hash()` (BLAKE3 of the JSON line),
    /// NOT by `event.hash` (the content/blob hash). Tests that want to assert
    /// on ledger presence must use this helper rather than comparing
    /// `event.hash` against directory listings.
    fn staged_event_exists(kin: &Path, event: &Event) -> bool {
        let h = event.event_hash().unwrap();
        staged_event_path(kin, &h).is_file()
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
        let res = commit_root(&root, "rfcs", params("yj", "2026-04-19T12:00:00Z")).unwrap();
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
        let res = commit_root(&root, "rfcs", params("yj", "2026-04-19T10:00:00Z")).unwrap();
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
        // First commit — produces a root with the entry. Then pin it.
        commit_root(&root, "rfcs", params("yj", "2026-04-11T10:00:00Z")).unwrap();
        overwrite_root_with_pin(&root, "rfcs", &old.id, &old.hash, "2026-04-11T10:00:01Z");
        // Second commit with a much later `now` — the 14-day-old entry
        // would normally drop, but pin should exempt it.
        let res = commit_root(&root, "rfcs", params("yj", "2026-04-19T10:00:00Z")).unwrap();
        let version = res.new_version.unwrap_or_else(|| res.prior_version.clone().unwrap());
        let ids = root_ids(&root, &version);
        assert_eq!(ids, vec![old.id], "pinned old entry must survive");
    }

    // -- Staged-ledger prune: MaxAge ----------------------------------------
    //
    // After kinora-wcpp, MaxAge retention no longer runs in staging — it
    // runs on the kinograph via apply_root_entry_gc reading entry.head_ts.
    // Both old and fresh owned events are drained after archive (same as
    // Never); the old *entry* is dropped from the kinograph by GC.

    #[test]
    fn max_age_drains_both_old_and_fresh_owned_events() {
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

        commit_root(&root, "rfcs", params("yj", "2026-04-19T10:00:00Z")).unwrap();
        // Both drained — retention moves to the kinograph.
        assert!(!staged_event_exists(&root, &old), "old store event drained");
        assert!(!staged_event_exists(&root, &fresh), "fresh store event drained");
    }

    // -- Staged-ledger prune: KeepLastN -------------------------------------

    #[test]
    fn keep_last_n_keeps_only_n_most_recent_staged_events_per_kino() {
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
        commit_root(&root, "rfcs", params("yj", "2026-04-19T10:00:00Z")).unwrap();

        // v3, v4, v5 survive; v1, v2 get pruned.
        assert!(!staged_event_exists(&root, &chain[0]), "v1 should be pruned");
        assert!(!staged_event_exists(&root, &chain[1]), "v2 should be pruned");
        assert!(staged_event_exists(&root, &chain[2]), "v3 should survive");
        assert!(staged_event_exists(&root, &chain[3]), "v4 should survive");
        assert!(staged_event_exists(&root, &chain[4]), "v5 should survive");
    }

    #[test]
    fn keep_last_n_pin_on_version_1_survives_plus_three_newest() {
        let (_t, root) = setup();
        // Start with keep-last-5 so the first commit materializes every
        // version without pruning anything (Never would prune-after-archive
        // under kinora-bayr) — the user can then hand-edit the pin.
        write_config(
            &root,
            r#"repo-url "https://example.com/x.git"
roots {
  rfcs { policy "keep-last-5" }
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
        commit_root(&root, "rfcs", params("yj", "2026-04-05T11:00:00Z")).unwrap();
        // Pin the root entry to v1 explicitly. This simulates a hand-edit.
        overwrite_root_with_pin(&root, "rfcs", &chain[0].id, &chain[0].hash, "2026-04-05T11:30:00Z");
        // Now switch the policy to keep-last-3 and run again. The pin from
        // the prior root must propagate and protect v1 from the N-window.
        write_config(
            &root,
            r#"repo-url "https://example.com/x.git"
roots {
  rfcs { policy "keep-last-3" }
}
"#,
        );
        commit_root(&root, "rfcs", params("yj", "2026-04-19T10:00:00Z")).unwrap();
        // v1 pinned → survives; v3, v4, v5 are the 3 newest → survive; v2 pruned.
        assert!(staged_event_exists(&root, &chain[0]), "v1 pinned, must survive");
        assert!(!staged_event_exists(&root, &chain[1]), "v2 not in top-3, not pinned → pruned");
        assert!(staged_event_exists(&root, &chain[2]), "v3 in top-3, survives");
        assert!(staged_event_exists(&root, &chain[3]), "v4 in top-3, survives");
        assert!(staged_event_exists(&root, &chain[4]), "v5 in top-3, survives");
    }

    // -- Staged-ledger prune baseline: fresh events untouched ----------------

    #[test]
    fn fresh_staged_events_untouched_by_keep_last_n_policy() {
        // KeepLastN retention still runs on staging (its "N versions per
        // kino" semantic is load-bearing on the staged stream). Fresh
        // events within the N-window must survive commit. After wcpp,
        // MaxAge no longer has this invariant — covered by the drain tests.
        let (_t, root) = setup();
        write_config(
            &root,
            r#"repo-url "https://example.com/x.git"
roots {
  rfcs { policy "keep-last-10" }
}
"#,
        );
        let a = store_md(&root, b"a", "a", "2026-04-18T10:00:00Z");
        let b = store_md(&root, b"b", "b", "2026-04-19T09:00:00Z");
        write_assign_for(&root, &a.id, "rfcs", vec![], "2026-04-18T10:00:01Z");
        write_assign_for(&root, &b.id, "rfcs", vec![], "2026-04-19T09:00:01Z");
        let count_before = staged_event_count(&root);
        commit_root(&root, "rfcs", params("yj", "2026-04-19T10:00:00Z")).unwrap();
        let count_after = staged_event_count(&root);
        assert!(
            count_after >= count_before,
            "fresh events must survive under KeepLastN: before={count_before}, after={count_after}"
        );
        assert!(staged_event_exists(&root, &a));
        assert!(staged_event_exists(&root, &b));
    }

    // -- Ownership wins over pin on cross-root reassign -----------------

    #[test]
    fn pin_in_root_a_is_dropped_when_kino_is_reassigned_to_root_b() {
        let (_t, root) = setup();
        // keep-last-10 instead of Never: the test is about pin-drop on
        // cross-root reassign, not about the Never prune path. Never would
        // drop the kino's store event on rfcs' first commit, leaving
        // nothing for designs to materialize.
        write_config(
            &root,
            r#"repo-url "https://example.com/x.git"
roots {
  rfcs { policy "keep-last-10" }
  designs { policy "keep-last-10" }
}
"#,
        );
        let k = store_md(&root, b"v", "v", "2024-04-19T10:00:00Z");
        let first = write_assign_for(&root, &k.id, "rfcs", vec![], "2024-04-19T10:00:01Z");
        commit_root(&root, "rfcs", params("yj", "2026-04-19T10:00:00Z")).unwrap();
        overwrite_root_with_pin(&root, "rfcs", &k.id, &k.hash, "2026-04-19T10:00:01Z");
        // Reassign to designs — this supersedes the rfcs assign.
        write_assign_for(
            &root,
            &k.id,
            "designs",
            vec![first.as_hex().to_owned()],
            "2026-04-19T11:00:00Z",
        );
        // Both roots run a commit. The pin on rfcs must not survive the
        // move — routing now puts the kino in designs, so rfcs' rebuild
        // has no entry for it at all.
        let rfcs = commit_root(&root, "rfcs", params("yj", "2026-04-19T12:00:00Z")).unwrap();
        let designs = commit_root(&root, "designs", params("yj", "2026-04-19T12:00:00Z")).unwrap();
        let rfcs_ids = root_ids(&root, &rfcs.new_version.unwrap());
        let designs_ids = root_ids(
            &root,
            &designs.new_version.expect("designs must materialize"),
        );
        assert!(rfcs_ids.is_empty(), "rfcs lost ownership; pin must drop");
        assert_eq!(designs_ids, vec![k.id], "designs now owns the kino");
    }

    // -- MaxAge prunes old assigns alongside old store events -----------

    #[test]
    fn max_age_drains_both_old_and_fresh_assign_events() {
        let (_t, root) = setup();
        write_config(
            &root,
            r#"repo-url "https://example.com/x.git"
roots {
  rfcs { policy "7d" }
}
"#,
        );
        let old = store_md(&root, b"old", "old", "2026-04-05T10:00:00Z");
        let fresh = store_md(&root, b"fresh", "fresh", "2026-04-18T10:00:00Z");
        let old_assign_hash =
            write_assign_for(&root, &old.id, "rfcs", vec![], "2026-04-05T10:00:01Z");
        let fresh_assign_hash =
            write_assign_for(&root, &fresh.id, "rfcs", vec![], "2026-04-18T10:00:01Z");

        let events = Ledger::new(&root).read_all_events().unwrap();
        let old_assign = events
            .iter()
            .find(|e| e.event_hash().unwrap() == old_assign_hash)
            .expect("old assign present")
            .clone();
        let fresh_assign = events
            .iter()
            .find(|e| e.event_hash().unwrap() == fresh_assign_hash)
            .expect("fresh assign present")
            .clone();

        commit_root(&root, "rfcs", params("yj", "2026-04-19T10:00:00Z")).unwrap();

        assert!(!staged_event_exists(&root, &old_assign), "old assign drained");
        assert!(!staged_event_exists(&root, &fresh_assign), "fresh assign drained");
    }

    // ------------------------------------------------------------------
    // f0rg: cross-root integrity (external refs prevent GC drops)
    // ------------------------------------------------------------------

    use crate::kinograph::{Entry as KinographEntry, Kinograph};

    /// Store a kinograph-kind kino composing the given entries. Returns
    /// the `Event` for the composition's store event.
    fn store_kinograph(
        kin: &Path,
        entries: Vec<KinographEntry>,
        name: &str,
        ts: &str,
    ) -> Event {
        let k = Kinograph { entries };
        let content = k.to_styx().unwrap().into_bytes();
        let stored = store_kino(
            kin,
            StoreKinoParams {
                kind: "kinograph".into(),
                content,
                author: "yj".into(),
                provenance: "f0rg-test".into(),
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
    fn cross_root_ref_from_a_prevents_b_gc_from_dropping_referenced_version() {
        let (_t, root) = setup();
        write_config(
            &root,
            r#"repo-url "https://example.com/x.git"
roots {
  rfcs { policy "never" }
  inbox-b { policy "30d" }
}
"#,
        );
        // X is 40 days old. Policy on inbox-b is 30d, so X would normally
        // drop. But a kinograph in rfcs composes X — integrity must save it.
        let x = store_md(&root, b"x body", "x", "2026-03-10T10:00:00Z");
        write_assign_for(&root, &x.id, "inbox-b", vec![], "2026-03-10T10:00:01Z");
        // Composition kinograph in rfcs pins to X's specific version.
        let kg_entry = KinographEntry {
            id: x.id.clone(),
            name: String::new(),
            pin: x.hash.clone(),
            note: String::new(),
        };
        let kg = store_kinograph(&root, vec![kg_entry], "my-list", "2026-04-10T10:00:00Z");
        write_assign_for(&root, &kg.id, "rfcs", vec![], "2026-04-10T10:00:01Z");

        // Commit both roots. B's 30d policy would drop X, but rfcs' kg
        // references it → integrity holds X.
        let rfcs_res =
            commit_root(&root, "rfcs", params("yj", "2026-04-19T12:00:00Z")).unwrap();
        let b_res =
            commit_root(&root, "inbox-b", params("yj", "2026-04-19T12:00:00Z")).unwrap();
        let rfcs_ids = root_ids(&root, &rfcs_res.new_version.unwrap());
        let b_ids = root_ids(&root, &b_res.new_version.unwrap());
        assert_eq!(rfcs_ids, vec![kg.id], "rfcs keeps its kinograph");
        assert_eq!(
            b_ids,
            vec![x.id.clone()],
            "inbox-b must NOT drop X — rfcs composes it"
        );
        // Under wcpp, owned events archive + drain on commit — so X's
        // staged event is gone, but the archive kino preserves it and
        // the root entry survives via cross-root protection. That's what
        // integrity means post-wcpp: the entry stays in B's kinograph,
        // not that the raw event persists in staging.
    }

    #[test]
    fn removing_cross_root_reference_allows_subsequent_gc_drop() {
        let (_t, root) = setup();
        write_config(
            &root,
            r#"repo-url "https://example.com/x.git"
roots {
  rfcs { policy "never" }
  inbox-b { policy "30d" }
}
"#,
        );
        let x = store_md(&root, b"x body", "x", "2026-03-10T10:00:00Z");
        write_assign_for(&root, &x.id, "inbox-b", vec![], "2026-03-10T10:00:01Z");
        let y = store_md(&root, b"y body", "y", "2026-04-10T09:00:00Z");
        write_assign_for(&root, &y.id, "inbox-b", vec![], "2026-04-10T09:00:01Z");

        // v1 of the kinograph references X.
        let kg_v1 = store_kinograph(
            &root,
            vec![KinographEntry {
                id: x.id.clone(),
                name: String::new(),
                pin: x.hash.clone(),
                note: String::new(),
            }],
            "my-list",
            "2026-04-10T10:00:00Z",
        );
        write_assign_for(&root, &kg_v1.id, "rfcs", vec![], "2026-04-10T10:00:01Z");
        commit_root(&root, "rfcs", params("yj", "2026-04-11T10:00:00Z")).unwrap();
        commit_root(&root, "inbox-b", params("yj", "2026-04-11T10:00:00Z")).unwrap();

        // v2 of the kinograph replaces X with Y.
        let kg_v2_k = Kinograph {
            entries: vec![KinographEntry {
                id: y.id.clone(),
                name: String::new(),
                pin: y.hash.clone(),
                note: String::new(),
            }],
        };
        let stored = store_kino(
            &root,
            StoreKinoParams {
                kind: "kinograph".into(),
                content: kg_v2_k.to_styx().unwrap().into_bytes(),
                author: "yj".into(),
                provenance: "f0rg-test".into(),
                ts: "2026-04-15T10:00:00Z".into(),
                metadata: BTreeMap::from([("name".into(), "my-list".into())]),
                id: Some(kg_v1.id.clone()),
                parents: vec![kg_v1.hash.clone()],
            },
        )
        .unwrap();
        let _kg_v2 = stored.event;
        // The kg_v1→rfcs assign was drained on first rfcs commit. Write
        // a fresh assign so kg_v2 routes to rfcs and rfcs v2 replaces
        // kg_v1 with kg_v2 (which references Y instead of X).
        write_assign_for(&root, &kg_v1.id, "rfcs", vec![], "2026-04-15T10:00:01Z");

        // Commit both roots again. rfcs now refs Y, not X. X is ONLY
        // owned by inbox-b, 40d old, no cross-root protection → drop.
        let rfcs2 =
            commit_root(&root, "rfcs", params("yj", "2026-04-19T12:00:00Z")).unwrap();
        assert!(
            rfcs2.new_version.is_some(),
            "rfcs v2 must advance to kg_v2 (Y ref)"
        );
        let b_res =
            commit_root(&root, "inbox-b", params("yj", "2026-04-19T12:00:00Z")).unwrap();
        let b_ids = root_ids(&root, &b_res.new_version.unwrap());
        assert!(
            !b_ids.contains(&x.id),
            "X no longer referenced; must drop under 30d policy"
        );
        assert!(b_ids.contains(&y.id), "Y is fresh and still referenced");
    }

    #[test]
    fn circular_cross_root_references_do_not_loop() {
        let (_t, root) = setup();
        write_config(
            &root,
            r#"repo-url "https://example.com/x.git"
roots {
  ra { policy "30d" }
  rb { policy "30d" }
}
"#,
        );
        // Two kinographs, each in a different root, each eventually
        // referencing the other. We build them in two stages because the
        // first one's id isn't known until it's stored.
        let stub = store_md(&root, b"stub", "stub", "2026-04-10T10:00:00Z");
        let kg_a = store_kinograph(
            &root,
            vec![KinographEntry {
                id: stub.id.clone(),
                name: String::new(),
                pin: stub.hash.clone(),
                note: String::new(),
            }],
            "kg-a",
            "2026-04-10T10:00:01Z",
        );
        write_assign_for(&root, &kg_a.id, "ra", vec![], "2026-04-10T10:00:02Z");
        let kg_b = store_kinograph(
            &root,
            vec![KinographEntry {
                id: kg_a.id.clone(),
                name: String::new(),
                pin: kg_a.hash.clone(),
                note: String::new(),
            }],
            "kg-b",
            "2026-04-10T10:00:03Z",
        );
        write_assign_for(&root, &kg_b.id, "rb", vec![], "2026-04-10T10:00:04Z");
        // Now create a second version of kg_a that references kg_b — closes the cycle.
        let kg_a_v2_content = Kinograph {
            entries: vec![KinographEntry {
                id: kg_b.id.clone(),
                name: String::new(),
                pin: kg_b.hash.clone(),
                note: String::new(),
            }],
        };
        store_kino(
            &root,
            StoreKinoParams {
                kind: "kinograph".into(),
                content: kg_a_v2_content.to_styx().unwrap().into_bytes(),
                author: "yj".into(),
                provenance: "f0rg-test".into(),
                ts: "2026-04-10T10:00:05Z".into(),
                metadata: BTreeMap::from([("name".into(), "kg-a".into())]),
                id: Some(kg_a.id.clone()),
                parents: vec![kg_a.hash.clone()],
            },
        )
        .unwrap();

        // Both commits must terminate cleanly.
        let a_res = commit_root(&root, "ra", params("yj", "2026-04-19T12:00:00Z")).unwrap();
        let b_res = commit_root(&root, "rb", params("yj", "2026-04-19T12:00:00Z")).unwrap();
        assert!(a_res.new_version.is_some());
        assert!(b_res.new_version.is_some());
    }

    #[test]
    fn commit_all_snapshot_taken_at_batch_start_protects_across_roots() {
        // Verifies that commit_all computes its ExternalRefs snapshot
        // once and passes it to every per-root commit — even the root
        // whose own commit bumps another root's pointer.
        let (_t, root) = setup();
        write_config(
            &root,
            r#"repo-url "https://example.com/x.git"
roots {
  rfcs { policy "never" }
  inbox-b { policy "30d" }
}
"#,
        );
        let x = store_md(&root, b"x body", "x", "2026-03-10T10:00:00Z");
        write_assign_for(&root, &x.id, "inbox-b", vec![], "2026-03-10T10:00:01Z");
        let kg = store_kinograph(
            &root,
            vec![KinographEntry {
                id: x.id.clone(),
                name: String::new(),
                pin: x.hash.clone(),
                note: String::new(),
            }],
            "my-list",
            "2026-04-10T10:00:00Z",
        );
        write_assign_for(&root, &kg.id, "rfcs", vec![], "2026-04-10T10:00:01Z");

        // Pre-commit rfcs once so its root pointer names the kg
        // kinograph; otherwise the batch snapshot wouldn't yet see the
        // reference.
        commit_root(&root, "rfcs", params("yj", "2026-04-11T10:00:00Z")).unwrap();

        let entries = commit_all(&root, params("yj", "2026-04-19T12:00:00Z")).unwrap();
        let by_name: std::collections::HashMap<_, _> = entries.into_iter().collect();
        let b_res = by_name["inbox-b"].as_ref().unwrap();
        let b_ids = root_ids(&root, &b_res.new_version.as_ref().unwrap().clone());
        assert!(
            b_ids.contains(&x.id),
            "commit_all must propagate cross-root protection: {b_ids:?}"
        );
        assert_eq!(
            b_res.retained_by_cross_root.get("rfcs").copied(),
            Some(1),
            "retention report names rfcs as the protector"
        );
    }

    #[test]
    fn overlapping_refs_from_two_roots_both_count_in_retention() {
        // A single entry in root C is referenced by kinographs in both
        // root A and root B. The retention report accumulates a count
        // per referencing root — so the same entry shows as retained by
        // both A (1) and B (1), and the total is 2, not 1.
        let (_t, root) = setup();
        write_config(
            &root,
            r#"repo-url "https://example.com/x.git"
roots {
  ra { policy "never" }
  rb { policy "never" }
  rc { policy "30d" }
}
"#,
        );
        let x = store_md(&root, b"x body", "x", "2026-03-10T10:00:00Z");
        write_assign_for(&root, &x.id, "rc", vec![], "2026-03-10T10:00:01Z");
        // Distinct note fields keep the two kinographs' byte content
        // apart, so they hash to different kino ids (otherwise the
        // assign graph would see the same kino pointed at two roots).
        let kg_a = store_kinograph(
            &root,
            vec![KinographEntry {
                id: x.id.clone(),
                name: String::new(),
                pin: x.hash.clone(),
                note: "ra-list".into(),
            }],
            "kg-a",
            "2026-04-10T10:00:00Z",
        );
        write_assign_for(&root, &kg_a.id, "ra", vec![], "2026-04-10T10:00:01Z");
        let kg_b = store_kinograph(
            &root,
            vec![KinographEntry {
                id: x.id.clone(),
                name: String::new(),
                pin: x.hash.clone(),
                note: "rb-list".into(),
            }],
            "kg-b",
            "2026-04-10T10:00:02Z",
        );
        write_assign_for(&root, &kg_b.id, "rb", vec![], "2026-04-10T10:00:03Z");

        commit_root(&root, "ra", params("yj", "2026-04-11T10:00:00Z")).unwrap();
        commit_root(&root, "rb", params("yj", "2026-04-11T10:00:00Z")).unwrap();
        let c_res =
            commit_root(&root, "rc", params("yj", "2026-04-19T12:00:00Z")).unwrap();
        assert_eq!(c_res.retained_by_cross_root.get("ra").copied(), Some(1));
        assert_eq!(c_res.retained_by_cross_root.get("rb").copied(), Some(1));
    }

    // ------------------------------------------------------------------
    // q6bo: per-commit archive kinos for the `commits` root
    // ------------------------------------------------------------------

    #[test]
    fn commit_root_produces_archive_kino_and_assign_to_commits() {
        // When a non-commits root actually promotes work, `commit_root`
        // stores a `commit-archive` kino and assigns it to `commits` so
        // the archive enters the commits kinograph on the next pass.
        let (_t, root) = setup();
        write_config(
            &root,
            r#"repo-url "https://example.com/x.git"
roots {
  main { policy "never" }
}
"#,
        );

        let k = store_md(&root, b"doc", "doc", "2026-04-19T10:00:00Z");
        write_assign_for(&root, &k.id, "main", vec![], "2026-04-19T10:00:01Z");

        let res =
            commit_root(&root, "main", params("yj", "2026-04-19T10:00:02Z")).unwrap();
        assert!(res.new_version.is_some(), "main should have produced a version");

        let events = Ledger::new(&root).read_all_events().unwrap();
        let archive_store = events
            .iter()
            .find(|e| e.is_store_event() && e.kind == ARCHIVE_CONTENT_KIND)
            .expect("archive store event should have been recorded");
        assert_eq!(
            archive_store.metadata.get("name").map(String::as_str),
            Some("main-commit-archive"),
            "archive name should be <root>-commit-archive",
        );

        let archive_assign = events
            .iter()
            .find(|e| {
                if e.event_kind != EVENT_KIND_ASSIGN {
                    return false;
                }
                let Ok(a) = AssignEvent::from_event(e) else {
                    return false;
                };
                a.kino_id == archive_store.id && a.target_root == COMMITS_ROOT
            })
            .expect("assign from archive kino to `commits` should exist");
        let _ = archive_assign;

        // Archive is a real blob in the content store — readable by hash.
        let hash = Hash::from_str(&archive_store.hash).unwrap();
        let bytes = ContentStore::new(&root).read(&hash).unwrap();
        let (schema, parsed) =
            crate::commit_archive::parse_archive(&bytes).unwrap();
        assert_eq!(schema, crate::commit_archive::ARCHIVE_SCHEMA_V1);
        // The archive body captured main's owned events: the store of
        // `doc` and the assign routing it to `main`.
        assert!(
            parsed.iter().any(|e| e.is_store_event() && e.id == k.id),
            "archive should contain the owned store event",
        );
        assert!(
            parsed.iter().any(|e| {
                if e.event_kind != EVENT_KIND_ASSIGN {
                    return false;
                }
                let Ok(a) = AssignEvent::from_event(e) else {
                    return false;
                };
                a.kino_id == k.id && a.target_root == "main"
            }),
            "archive should contain the owned assign event",
        );
    }

    #[test]
    fn commits_root_does_not_archive_itself() {
        // The commits root never produces its own archive — it would just
        // duplicate entries already present in its own kinograph.
        let (_t, root) = setup();
        write_config(
            &root,
            r#"repo-url "https://example.com/x.git"
roots {
  main { policy "never" }
}
"#,
        );
        let k = store_md(&root, b"m", "m", "2026-04-19T10:00:00Z");
        write_assign_for(&root, &k.id, "main", vec![], "2026-04-19T10:00:01Z");

        let entries = commit_all(&root, params("yj", "2026-04-19T10:00:02Z")).unwrap();

        // Exactly one archive entry in the commits kinograph (main's). A
        // second archive would mean commits tried to archive its own
        // version bump. kinora-bayr prunes the archive store event from
        // staging on the commits root's own Never commit, so the proof of
        // "how many archives got made" lives in the commits kinograph.
        let commits_version = entries
            .iter()
            .find(|(n, _)| n == COMMITS_ROOT)
            .unwrap()
            .1
            .as_ref()
            .unwrap()
            .new_version
            .clone()
            .unwrap();
        let commits_bytes =
            ContentStore::new(&root).read(&commits_version).unwrap();
        let commits_kg = RootKinograph::parse(&commits_bytes).unwrap();
        assert_eq!(
            commits_kg.entries.len(),
            1,
            "exactly one archive entry (main's) should exist; got: {:?}",
            commits_kg.entries,
        );
        assert_eq!(
            commits_kg.entries[0]
                .metadata
                .get("name")
                .map(String::as_str),
            Some("main-commit-archive"),
        );
    }

    #[test]
    fn commits_kinograph_contains_archive_after_commit_all() {
        // End-to-end: one batch of `commit_all` promotes a kino into main,
        // writes main's archive, and the commits root commits last —
        // picking the archive up as its own entry.
        let (_t, root) = setup();
        write_config(
            &root,
            r#"repo-url "https://example.com/x.git"
roots {
  main { policy "never" }
}
"#,
        );
        let k = store_md(&root, b"hello", "hello", "2026-04-19T10:00:00Z");
        write_assign_for(&root, &k.id, "main", vec![], "2026-04-19T10:00:01Z");

        let entries = commit_all(&root, params("yj", "2026-04-19T10:00:02Z")).unwrap();
        let commits_entry = entries
            .iter()
            .find(|(n, _)| n == COMMITS_ROOT)
            .expect("commits entry present");
        let commits_version = commits_entry
            .1
            .as_ref()
            .unwrap()
            .new_version
            .as_ref()
            .expect("commits should have a version — it consumed an archive-assign");

        // Under kinora-bayr the archive store event is pruned from staging
        // on commits' own Never commit — its presence in the commits
        // kinograph is the durable record. Assert the kinograph has exactly
        // one entry and that its content is a commit-archive (by reading
        // the blob through the content store).
        let commits_bytes =
            ContentStore::new(&root).read(commits_version).unwrap();
        let commits_kg = RootKinograph::parse(&commits_bytes).unwrap();
        assert_eq!(
            commits_kg.entries.len(),
            1,
            "commits kinograph should list exactly one archive entry; got: {:?}",
            commits_kg.entries,
        );
        let entry = &commits_kg.entries[0];
        let entry_version = Hash::from_str(&entry.version).unwrap();
        let archive_bytes =
            ContentStore::new(&root).read(&entry_version).unwrap();
        // parse_archive confirms the entry points at a valid commit-archive
        // kino; if this fails we promoted something unexpected into commits.
        parse_archive(&archive_bytes)
            .expect("commits entry should point at a valid commit-archive");
    }

    #[test]
    fn successive_commits_with_new_activity_stack_distinct_archives() {
        // Opposite of the idempotency case: run 1 commits one kino, run 2
        // stores and commits a second kino. Each run produces its own
        // archive (different contents → different hashes → two live
        // assigns into commits), and the commits kinograph lists both.
        let (_t, root) = setup();
        write_config(
            &root,
            r#"repo-url "https://example.com/x.git"
roots {
  main { policy "never" }
}
"#,
        );

        let k1 = store_md(&root, b"one", "one", "2026-04-19T10:00:00Z");
        write_assign_for(&root, &k1.id, "main", vec![], "2026-04-19T10:00:01Z");
        commit_all(&root, params("yj", "2026-04-19T10:00:02Z")).unwrap();

        // Second round: new user activity in main.
        let k2 = store_md(&root, b"two", "two", "2026-04-19T11:00:00Z");
        write_assign_for(&root, &k2.id, "main", vec![], "2026-04-19T11:00:01Z");
        let entries = commit_all(&root, params("yj", "2026-04-19T11:00:02Z")).unwrap();

        // Under kinora-bayr, archive store events + archive-assigns get
        // pruned from staging once commits ingests them, so the commits
        // kinograph is the durable record of "how many distinct archives".
        // Two runs with distinct owned events → two distinct archive ids →
        // two entries in commits' kinograph.
        let commits_version = entries
            .iter()
            .find(|(n, _)| n == COMMITS_ROOT)
            .unwrap()
            .1
            .as_ref()
            .unwrap()
            .new_version
            .clone()
            .unwrap();
        let commits_bytes =
            ContentStore::new(&root).read(&commits_version).unwrap();
        let commits_kg = RootKinograph::parse(&commits_bytes).unwrap();
        let ids: std::collections::HashSet<_> =
            commits_kg.entries.iter().map(|e| e.id.clone()).collect();
        assert_eq!(
            ids.len(),
            2,
            "two runs with new activity should leave two archive entries in commits; got: {:?}",
            commits_kg.entries,
        );
        // Each entry must point at a valid commit-archive blob — the
        // content-store survives staging prune.
        for entry in &commits_kg.entries {
            let v = Hash::from_str(&entry.version).unwrap();
            let bytes = ContentStore::new(&root).read(&v).unwrap();
            parse_archive(&bytes)
                .expect("commits entry should point at a commit-archive blob");
        }
    }

    #[test]
    fn archive_creation_is_idempotent_across_repeated_commits() {
        // Running `commit_all` twice against the same effective state —
        // same owned events, same archive content — produces one archive
        // kino and one live assign into commits. Only timestamps differ
        // between runs; the archive hash is identical.
        let (_t, root) = setup();
        write_config(
            &root,
            r#"repo-url "https://example.com/x.git"
roots {
  main { policy "never" }
}
"#,
        );
        let k = store_md(&root, b"doc", "doc", "2026-04-19T10:00:00Z");
        write_assign_for(&root, &k.id, "main", vec![], "2026-04-19T10:00:01Z");

        let first = commit_all(&root, params("yj", "2026-04-19T10:00:02Z")).unwrap();
        // Second commit — no new user events happened, only time passed.
        let second = commit_all(&root, params("yj", "2026-04-19T11:00:00Z")).unwrap();

        // Run 1 produces a commits version with exactly one archive entry.
        let commits_v1 = first
            .iter()
            .find(|(n, _)| n == COMMITS_ROOT)
            .unwrap()
            .1
            .as_ref()
            .unwrap()
            .new_version
            .clone()
            .unwrap();
        let bytes_v1 = ContentStore::new(&root).read(&commits_v1).unwrap();
        let kg_v1 = RootKinograph::parse(&bytes_v1).unwrap();
        assert_eq!(kg_v1.entries.len(), 1);

        // Run 2 — same owned-event set on main (nothing new), same archive
        // hash. The commits root's rebuild sees no new activity either, so
        // its pointer must not advance: idempotent.
        let commits_v2 = second
            .iter()
            .find(|(n, _)| n == COMMITS_ROOT)
            .unwrap()
            .1
            .as_ref()
            .unwrap()
            .new_version
            .clone();
        assert!(
            commits_v2.is_none(),
            "re-running against the same state must not bump commits: got {commits_v2:?}",
        );
    }

    // ------------------------------------------------------------------
    // bayr: Never-policy prune + prior-root merge
    // ------------------------------------------------------------------

    #[test]
    fn never_policy_drains_owned_staged_events_after_archive() {
        // Under Never policy, commit must archive the owned events AND
        // remove them from staging (previously a no-op, causing
        // unbounded ledger growth).
        let (_t, root) = setup();
        write_config(
            &root,
            r#"repo-url "https://example.com/x.git"
roots {
  main { policy "never" }
}
"#,
        );
        let k = store_md(&root, b"doc", "doc", "2026-04-19T10:00:00Z");
        let k_assign_hash =
            write_assign_for(&root, &k.id, "main", vec![], "2026-04-19T10:00:01Z");
        let before = Ledger::new(&root).read_all_events().unwrap();
        let k_assign = before
            .iter()
            .find(|e| e.event_hash().unwrap() == k_assign_hash)
            .cloned()
            .unwrap();
        assert!(staged_event_exists(&root, &k));
        assert!(staged_event_exists(&root, &k_assign));

        commit_root(&root, "main", params("yj", "2026-04-19T10:00:02Z")).unwrap();

        assert!(
            !staged_event_exists(&root, &k),
            "store event must be pruned after Never archive",
        );
        assert!(
            !staged_event_exists(&root, &k_assign),
            "assign event must be pruned after Never archive",
        );
        // The archive store event for the commits root is still live —
        // only commits' own commit step can drop it.
        let events_after = Ledger::new(&root).read_all_events().unwrap();
        assert!(
            events_after
                .iter()
                .any(|e| e.is_store_event() && e.kind == ARCHIVE_CONTENT_KIND),
            "archive store event must still be in staging for commits to consume",
        );
    }

    #[test]
    fn commits_root_drains_owned_staged_events_after_commit_all() {
        // After commit_all, the commits root consumed the archive-assign
        // and the archive store event. Both should be drained from
        // staging. The commits kinograph still references the archive
        // kino by id.
        let (_t, root) = setup();
        write_config(
            &root,
            r#"repo-url "https://example.com/x.git"
roots {
  main { policy "never" }
}
"#,
        );
        let k = store_md(&root, b"doc", "doc", "2026-04-19T10:00:00Z");
        write_assign_for(&root, &k.id, "main", vec![], "2026-04-19T10:00:01Z");

        commit_all(&root, params("yj", "2026-04-19T10:00:02Z")).unwrap();

        let events = Ledger::new(&root).read_all_events().unwrap();
        let staged_archives: Vec<_> = events
            .iter()
            .filter(|e| e.is_store_event() && e.kind == ARCHIVE_CONTENT_KIND)
            .collect();
        assert!(
            staged_archives.is_empty(),
            "archive store events must be pruned; got: {staged_archives:?}",
        );
        let staged_commit_assigns: Vec<_> = events
            .iter()
            .filter(|e| {
                if e.event_kind != EVENT_KIND_ASSIGN {
                    return false;
                }
                let Ok(a) = AssignEvent::from_event(e) else {
                    return false;
                };
                a.target_root == COMMITS_ROOT
            })
            .collect();
        assert!(
            staged_commit_assigns.is_empty(),
            "archive-assigns must be pruned; got: {staged_commit_assigns:?}",
        );
        // Commits kinograph still has the archive entry.
        let pointer = read_root_pointer(&root, COMMITS_ROOT).unwrap().unwrap();
        let entries = root_ids(&root, &pointer);
        assert_eq!(
            entries.len(),
            1,
            "commits kinograph must retain the archive entry",
        );
    }

    #[test]
    fn never_policy_rebuild_preserves_prior_entries_after_prune() {
        // After Never-policy prune, the next commit (with no new events)
        // must preserve the prior kinograph's entries via prior_root
        // merge in build_root.
        let (_t, root) = setup();
        write_config(
            &root,
            r#"repo-url "https://example.com/x.git"
roots {
  main { policy "never" }
}
"#,
        );
        let k = store_md(&root, b"v1", "doc", "2026-04-19T10:00:00Z");
        write_assign_for(&root, &k.id, "main", vec![], "2026-04-19T10:00:01Z");

        let first = commit_root(&root, "main", params("yj", "2026-04-19T10:00:02Z")).unwrap();
        assert_eq!(
            root_ids(&root, &first.new_version.clone().unwrap()),
            vec![k.id.clone()],
        );

        let second =
            commit_root(&root, "main", params("yj", "2026-04-19T11:00:00Z")).unwrap();
        assert!(
            second.new_version.is_none(),
            "no new events → rebuild must match prior → no-op",
        );
        let pointer = read_root_pointer(&root, "main").unwrap().unwrap();
        assert_eq!(
            root_ids(&root, &pointer),
            vec![k.id],
            "entry must be preserved via prior_root merge",
        );
    }

    #[test]
    fn never_policy_reassign_removes_entry_on_next_rebuild() {
        // Kino committed to Never root A, pruned, then reassigned to
        // root B. On next commit of A, the kino must be gone from A's
        // kinograph — the prior_root merge must respect live reassigns.
        let (_t, root) = setup();
        write_config(
            &root,
            r#"repo-url "https://example.com/x.git"
roots {
  main { policy "never" }
  alt { policy "never" }
}
"#,
        );
        let k = store_md(&root, b"v", "doc", "2026-04-19T10:00:00Z");
        write_assign_for(&root, &k.id, "main", vec![], "2026-04-19T10:00:01Z");

        commit_root(&root, "main", params("yj", "2026-04-19T10:00:02Z")).unwrap();

        // The old assign is pruned. A brand-new assign to `alt` is the
        // only live assign for this kino after pruning.
        write_assign_for(&root, &k.id, "alt", vec![], "2026-04-19T11:00:00Z");

        let second =
            commit_root(&root, "main", params("yj", "2026-04-19T11:00:01Z")).unwrap();
        let new_version = second
            .new_version
            .expect("reassignment away should produce a new main version");
        let ids = root_ids(&root, &new_version);
        assert!(
            !ids.contains(&k.id),
            "reassigned kino must NOT merge back into main: {ids:?}",
        );
    }

    #[test]
    fn build_root_prior_merge_preserves_pinned_entry_with_no_staged_events() {
        // Unit-level: build_root(&[], …, Some(prior_with_pinned_entry))
        // must keep the pinned entry intact.
        let declared: BTreeSet<String> = BTreeSet::from(["main".to_owned()]);
        let pinned = RootEntry {
            id: "a".repeat(64),
            version: "b".repeat(64),
            kind: "markdown".into(),
            metadata: BTreeMap::from([("name".into(), "pinned".into())]),
            note: String::new(),
            pin: true,
            head_ts: String::new(),
        };
        let prior = RootKinograph { entries: vec![pinned.clone()] };
        let built = build_root(&[], "main", &declared, Some(&prior)).unwrap();
        assert_eq!(built.entries, vec![pinned]);
    }

    #[test]
    fn build_root_prior_merge_respects_live_reassign() {
        // Prior root has kino X. Staged events contain a live assign
        // reassigning X to "other". Merge must NOT resurrect X under this
        // root.
        let declared: BTreeSet<String> =
            BTreeSet::from(["main".to_owned(), "other".to_owned()]);
        let x_id = "a".repeat(64);
        let prior = RootKinograph {
            entries: vec![RootEntry::new(
                x_id.clone(),
                "b".repeat(64),
                "markdown",
                BTreeMap::new(),
                "",
            )],
        };
        let reassign = AssignEvent {
            kino_id: x_id.clone(),
            target_root: "other".into(),
            supersedes: vec![],
            author: "yj".into(),
            ts: "2026-04-19T11:00:00Z".into(),
            provenance: "test".into(),
        }
        .to_event();
        let built =
            build_root(&[reassign], "main", &declared, Some(&prior)).unwrap();
        assert!(
            built.entries.is_empty(),
            "reassigned kino must not merge back: {:?}",
            built.entries,
        );
    }

    #[test]
    fn build_root_prior_merge_does_not_duplicate_entry_already_in_fresh() {
        // When both fresh build and prior have the same id, merge must
        // not add a duplicate — propagate_pins handles pin propagation
        // for entries in the fresh build.
        let declared: BTreeSet<String> = BTreeSet::from(["main".to_owned()]);
        // Store event for X on `main` (default routing, no assign).
        let x_hash = crate::hash::Hash::of_content(b"x-body");
        let store = Event::new_store(
            "markdown".into(),
            x_hash.as_hex().into(),
            x_hash.as_hex().into(),
            vec![],
            "2026-04-19T10:00:00Z".into(),
            "yj".into(),
            "test".into(),
            BTreeMap::from([("name".into(), "x".into())]),
        );
        let declared_with_inbox: BTreeSet<String> =
            BTreeSet::from(["inbox".to_owned()]);
        let prior = RootKinograph {
            entries: vec![RootEntry::new(
                store.id.clone(),
                "stale-version",
                "markdown",
                BTreeMap::new(),
                "",
            )],
        };
        let built = build_root(
            std::slice::from_ref(&store),
            "inbox",
            &declared_with_inbox,
            Some(&prior),
        )
        .unwrap();
        assert_eq!(built.entries.len(), 1);
        // Fresh build wins the version, since the prior entry was not pinned.
        assert_eq!(built.entries[0].version, store.hash);
        let _ = declared;
    }

    // ------------------------------------------------------------------
    // 0sgr: head_ts on RootEntry makes GC independent of staging
    // ------------------------------------------------------------------

    #[test]
    fn build_root_populates_head_ts_from_head_event() {
        // build_root must copy the head store event's ts onto RootEntry,
        // so GC can age entries out without consulting the staged stream.
        let (_t, root) = setup();
        let k = store_md(&root, b"hello", "hello", "2026-04-10T12:34:56Z");
        let declared: BTreeSet<String> = BTreeSet::from(["inbox".to_owned()]);
        let events = Ledger::new(&root).read_all_events().unwrap();
        let built = build_root(&events, "inbox", &declared, None).unwrap();
        let entry = built
            .entries
            .iter()
            .find(|e| e.id == k.id)
            .expect("entry for stored kino");
        assert_eq!(
            entry.head_ts, "2026-04-10T12:34:56Z",
            "entry.head_ts must match the head store event's ts",
        );
    }

    #[test]
    fn entry_gc_uses_head_ts_on_entry_not_staged_event() {
        // Construct a root kinograph with a synthetic entry whose
        // head_ts is old. Run apply_root_entry_gc with an *empty* event
        // slice — meaning the head event is not in staging. GC must still
        // age the entry out based on entry.head_ts.
        let old_id = "a".repeat(64);
        let old_version = "b".repeat(64);
        let entry = RootEntry {
            id: old_id.clone(),
            version: old_version.clone(),
            kind: "markdown".into(),
            metadata: BTreeMap::new(),
            note: String::new(),
            pin: false,
            head_ts: "2026-04-05T10:00:00Z".into(), // 14 days older than `now`
        };
        let mut kg = RootKinograph { entries: vec![entry] };
        let policy = RootPolicy::MaxAge("7d".into());
        let implicit: BTreeSet<(String, String)> = BTreeSet::new();
        let refs = ExternalRefs::default();

        apply_root_entry_gc(
            &mut kg,
            "rfcs",
            &policy,
            "2026-04-19T10:00:00Z",
            &implicit,
            &refs,
        )
        .unwrap();

        assert!(
            kg.entries.is_empty(),
            "GC must drop entry whose head_ts is older than cutoff, even without staged head: {:?}",
            kg.entries,
        );
    }

    #[test]
    fn entry_gc_keeps_entry_when_head_ts_is_empty() {
        // Legacy entries loaded from pre-0sgr kinographs have an empty
        // head_ts. GC must conservatively keep them — matching the
        // pre-0sgr behavior where an unverifiable head meant "keep".
        let old_id = "a".repeat(64);
        let old_version = "b".repeat(64);
        let entry = RootEntry {
            id: old_id,
            version: old_version,
            kind: "markdown".into(),
            metadata: BTreeMap::new(),
            note: String::new(),
            pin: false,
            head_ts: String::new(),
        };
        let mut kg = RootKinograph { entries: vec![entry] };
        let policy = RootPolicy::MaxAge("7d".into());
        let implicit: BTreeSet<(String, String)> = BTreeSet::new();
        let refs = ExternalRefs::default();

        apply_root_entry_gc(
            &mut kg,
            "rfcs",
            &policy,
            "2026-04-19T10:00:00Z",
            &implicit,
            &refs,
        )
        .unwrap();

        assert_eq!(
            kg.entries.len(),
            1,
            "entry with empty head_ts must be kept (legacy fallback)",
        );
    }

    #[test]
    fn entry_gc_keeps_entry_when_head_ts_is_unparseable() {
        // Malformed head_ts (corrupt blob, partial write, etc.) must not
        // drop an entry — match the conservative keep-on-unverifiable
        // policy that applied when the same check was against staged events.
        let entry = RootEntry {
            id: "a".repeat(64),
            version: "b".repeat(64),
            kind: "markdown".into(),
            metadata: BTreeMap::new(),
            note: String::new(),
            pin: false,
            head_ts: "not-a-timestamp".into(),
        };
        let mut kg = RootKinograph { entries: vec![entry] };
        let policy = RootPolicy::MaxAge("7d".into());
        let implicit: BTreeSet<(String, String)> = BTreeSet::new();
        let refs = ExternalRefs::default();

        apply_root_entry_gc(
            &mut kg,
            "rfcs",
            &policy,
            "2026-04-19T10:00:00Z",
            &implicit,
            &refs,
        )
        .unwrap();

        assert_eq!(
            kg.entries.len(),
            1,
            "entry with unparseable head_ts must be kept, not dropped",
        );
    }

    // ------------------------------------------------------------------
    // wcpp: MaxAge drain + prior_root merge
    // ------------------------------------------------------------------

    #[test]
    fn max_age_drains_archived_events_from_staging_after_commit() {
        // Under MaxAge, owned store + assign events must be archived and
        // then drained from staging — regardless of age. Retention is now
        // the kinograph's job (via apply_root_entry_gc reading head_ts),
        // not staging's.
        let (_t, root) = setup();
        write_config(
            &root,
            r#"repo-url "https://example.com/x.git"
roots {
  rfcs { policy "30d" }
}
"#,
        );
        // Fresh (1-day-old) kino — well under the 30d cutoff. Previously
        // this would stay in staging; under wcpp it should be drained.
        let k = store_md(&root, b"doc", "doc", "2026-04-18T10:00:00Z");
        let k_assign_hash =
            write_assign_for(&root, &k.id, "rfcs", vec![], "2026-04-18T10:00:01Z");
        let before = Ledger::new(&root).read_all_events().unwrap();
        let k_assign = before
            .iter()
            .find(|e| e.event_hash().unwrap() == k_assign_hash)
            .cloned()
            .unwrap();
        assert!(staged_event_exists(&root, &k));
        assert!(staged_event_exists(&root, &k_assign));

        commit_root(&root, "rfcs", params("yj", "2026-04-19T10:00:00Z")).unwrap();

        assert!(
            !staged_event_exists(&root, &k),
            "store event must be drained after MaxAge archive",
        );
        assert!(
            !staged_event_exists(&root, &k_assign),
            "assign event must be drained after MaxAge archive",
        );
        // Archive store event is still in staging for commits to consume.
        let events_after = Ledger::new(&root).read_all_events().unwrap();
        assert!(
            events_after
                .iter()
                .any(|e| e.is_store_event() && e.kind == ARCHIVE_CONTENT_KIND),
            "archive store event must still be in staging for commits to consume",
        );
        // And the entry is in rfcs's kinograph.
        let pointer = read_root_pointer(&root, "rfcs").unwrap().unwrap();
        assert_eq!(root_ids(&root, &pointer), vec![k.id]);
    }

    #[test]
    fn max_age_entries_age_out_of_root_kinograph_via_gc_post_drain() {
        // After MaxAge drains owned events on commit, retention for old
        // entries must happen via apply_root_entry_gc reading entry.head_ts
        // — not via staging prune (which no longer fires for MaxAge).
        let (_t, root) = setup();
        write_config(
            &root,
            r#"repo-url "https://example.com/x.git"
roots {
  rfcs { policy "7d" }
}
"#,
        );
        // 14-day-old kino — will age out. A fresh one so the root has
        // something to keep around.
        let old = store_md(&root, b"old", "old", "2026-04-05T10:00:00Z");
        let fresh = store_md(&root, b"fresh", "fresh", "2026-04-18T10:00:00Z");
        write_assign_for(&root, &old.id, "rfcs", vec![], "2026-04-05T10:00:01Z");
        write_assign_for(&root, &fresh.id, "rfcs", vec![], "2026-04-18T10:00:01Z");

        // First commit: both staged, both archive, kinograph has `fresh`
        // (old entry drops immediately via GC because its head_ts is old).
        let res = commit_root(&root, "rfcs", params("yj", "2026-04-19T10:00:00Z"))
            .unwrap();
        let pointer = res.new_version.unwrap();
        let ids = root_ids(&root, &pointer);
        assert_eq!(
            ids,
            vec![fresh.id.clone()],
            "old entry must age out of kinograph on first commit after drain",
        );
        // Both store events and both assigns are drained.
        assert!(!staged_event_exists(&root, &old));
        assert!(!staged_event_exists(&root, &fresh));
    }

    #[test]
    fn max_age_prior_root_merges_entries_across_commits() {
        // Under MaxAge, once staged events are drained the prior kinograph
        // is the only record of entries. A subsequent commit with no new
        // activity must preserve them via the prior_root merge path in
        // build_root (analogous to the Never behavior under kinora-bayr).
        let (_t, root) = setup();
        write_config(
            &root,
            r#"repo-url "https://example.com/x.git"
roots {
  rfcs { policy "30d" }
}
"#,
        );
        let k = store_md(&root, b"v1", "doc", "2026-04-18T10:00:00Z");
        write_assign_for(&root, &k.id, "rfcs", vec![], "2026-04-18T10:00:01Z");

        let first = commit_root(&root, "rfcs", params("yj", "2026-04-19T10:00:00Z"))
            .unwrap();
        assert_eq!(
            root_ids(&root, &first.new_version.clone().unwrap()),
            vec![k.id.clone()],
        );

        // No new events — rebuild must see the same state via prior_root
        // merge (staged events have been drained).
        let second = commit_root(&root, "rfcs", params("yj", "2026-04-19T11:00:00Z"))
            .unwrap();
        assert!(
            second.new_version.is_none(),
            "no new events → rebuild must match prior → no-op",
        );
        let pointer = read_root_pointer(&root, "rfcs").unwrap().unwrap();
        assert_eq!(
            root_ids(&root, &pointer),
            vec![k.id],
            "entry must be preserved via prior_root merge after MaxAge drain",
        );
    }

    #[test]
    fn propagate_pins_keeps_head_ts_paired_with_version() {
        // When propagate_pins rewinds `entry.version` back to the prior
        // pinned version, `head_ts` must follow — otherwise the entry
        // reports the *fresh head's* ts while pointing at the prior
        // version's hash. That inconsistency would surface to any
        // downstream consumer (e.g. render via resolve synthesis) that
        // reads head_ts without first checking pin.
        let id = "a".repeat(64);
        let prior_version = "b".repeat(64);
        let fresh_version = "c".repeat(64);
        let prior = RootKinograph {
            entries: vec![RootEntry {
                id: id.clone(),
                version: prior_version.clone(),
                kind: "markdown".into(),
                metadata: BTreeMap::new(),
                note: String::new(),
                pin: true,
                head_ts: "2026-04-01T00:00:00Z".into(),
            }],
        };
        let mut fresh = RootKinograph {
            entries: vec![RootEntry {
                id: id.clone(),
                version: fresh_version.clone(),
                kind: "markdown".into(),
                metadata: BTreeMap::new(),
                note: String::new(),
                pin: false,
                head_ts: "2026-04-10T00:00:00Z".into(),
            }],
        };

        propagate_pins(&mut fresh, Some(&prior));

        let entry = &fresh.entries[0];
        assert!(entry.pin, "pin must propagate");
        assert_eq!(
            entry.version, prior_version,
            "version must rewind to prior pinned version",
        );
        assert_eq!(
            entry.head_ts, "2026-04-01T00:00:00Z",
            "head_ts must track the pinned version, not the fresh head",
        );
    }

    // ------------------------------------------------------------------
    // kinora-ojc8: drain_archived_orphans — migration-debt cleanup for
    // repos whose staging still carries events already recorded in a
    // commit-archive kino (happens when pre-wcpp binaries archived
    // without draining, or when later no-op commits can't fire the drain).
    // ------------------------------------------------------------------

    #[test]
    fn drain_archived_orphans_drops_staged_event_already_in_archive() {
        let (_t, root) = setup();
        // Stage + commit — wcpp archives the event into an archive kino
        // and drains staging. Capture the event up-front so we can
        // replay it into staging afterward.
        let e = store_md(&root, b"alpha", "alpha", "2026-04-20T10:00:00Z");
        commit_all(&root, params("yj", "2026-04-20T10:00:01Z")).unwrap();
        assert!(
            !staged_event_exists(&root, &e),
            "wcpp should have drained the store event post-archive",
        );

        // Simulate migration debt: pre-wcpp archived it but couldn't drain
        // from staging. Write the same event back.
        let ledger = Ledger::new(&root);
        let (_, was_new) = ledger.write_event(&e).unwrap();
        assert!(was_new, "replay should create a fresh staged file");
        assert!(staged_event_exists(&root, &e), "orphan should be present before drain");

        let dropped = drain_archived_orphans(&root).unwrap();
        assert_eq!(dropped, 1, "orphan event should be dropped");
        assert!(
            !staged_event_exists(&root, &e),
            "orphan event should be drained",
        );
    }

    #[test]
    fn drain_archived_orphans_preserves_unarchived_staged_events() {
        let (_t, root) = setup();
        // Stage a kino but do NOT commit — no archive is produced.
        let e = store_md(&root, b"solo", "solo", "2026-04-20T10:00:00Z");
        assert!(staged_event_exists(&root, &e), "staged event present before drain");

        let dropped = drain_archived_orphans(&root).unwrap();
        assert_eq!(dropped, 0, "nothing to drop without an archive");
        assert!(
            staged_event_exists(&root, &e),
            "unarchived event must survive drain",
        );
    }

    #[test]
    fn drain_archived_orphans_respects_keep_last_n_policy() {
        let (_t, root) = setup();
        write_config(
            &root,
            r#"repo-url "https://example.com/x.git"
roots {
  rfcs { policy "keep-last-5" }
}
"#,
        );
        // Stage + assign a kino to rfcs + commit. KeepLastN produces an
        // archive but does NOT drain staging post-archive (its retention
        // is staging-based, per wcpp). The archive will still reference
        // the staged event's hash — but drain_archived_orphans must
        // skip it because the source root is KeepLastN.
        let e = store_md(&root, b"rfc", "rfc", "2026-04-20T10:00:00Z");
        write_assign_for(&root, &e.id, "rfcs", vec![], "2026-04-20T10:00:01Z");
        commit_all(&root, params("yj", "2026-04-20T10:00:02Z")).unwrap();
        assert!(
            staged_event_exists(&root, &e),
            "KeepLastN keeps the event in staging (no wcpp drain)",
        );

        let dropped = drain_archived_orphans(&root).unwrap();
        assert_eq!(
            dropped, 0,
            "KeepLastN source root's archive entries must not be drained",
        );
        assert!(
            staged_event_exists(&root, &e),
            "KeepLastN retention must survive orphan drain",
        );
    }

    #[test]
    fn drain_archived_orphans_is_noop_when_commits_pointer_absent() {
        let (_t, root) = setup();
        // No commits have run — commits root pointer doesn't exist.
        let dropped = drain_archived_orphans(&root).unwrap();
        assert_eq!(dropped, 0, "no commits pointer → nothing to inspect");
    }
}
