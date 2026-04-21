//! Reformat legacy `.styx`-wrapped kinograph and root blobs into the new
//! styxl one-entry-per-line form.
//!
//! Strategy:
//!
//! 1. **Regular kinograph kinos** (`kind: "kinograph"`) reachable from any
//!    root's current root-kinograph entries are reformatted as *staged
//!    new-version events*. The reformat does not update pointers directly
//!    — the user's next `kinora commit` promotes the new versions to
//!    heads.
//! 2. **Root kinographs** (`kind: "root"`) are produced by commit, not
//!    staged. For those, reformat stores the new blob + records a store
//!    event + updates the root pointer in one step — the same shape that
//!    commit itself uses.
//! 3. Non-styx kinds (markdown/text/binary/…) are opaque byte streams and
//!    left untouched.
//! 4. Idempotent: re-running reformat on an already-styxl repo stages no
//!    events and updates no pointers.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io;
use std::path::Path;
use std::str::FromStr;

use crate::commit::{read_root_pointer, CommitError};
use crate::config::{Config, ConfigError};
use crate::event::{Event, EventError};
use crate::hash::{Hash, HashParseError};
use crate::kino::{store_kino, StoreKinoError, StoreKinoParams};
use crate::kinograph::{is_styxl, Kinograph, KinographError};
use crate::ledger::{Ledger, LedgerError};
use crate::paths::{config_path, root_pointer_path, roots_dir};
use crate::root::{RootError, RootKinograph};
use crate::store::{ContentStore, StoreError};

#[derive(Debug, thiserror::Error)]
pub enum ReformatError {
    #[error("reformat io error: {0}")]
    Io(#[from] io::Error),
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Ledger(#[from] LedgerError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    StoreKino(#[from] StoreKinoError),
    #[error(transparent)]
    Kinograph(#[from] KinographError),
    #[error(transparent)]
    Root(#[from] RootError),
    #[error(transparent)]
    Event(#[from] EventError),
    #[error(transparent)]
    Commit(#[from] CommitError),
    #[error("invalid hash `{value}`: {err}")]
    InvalidHash {
        value: String,
        #[source]
        err: HashParseError,
    },
    #[error("root pointer {name} references a version `{version}` with no matching store event")]
    PriorRootEventMissing { name: String, version: String },
    #[error("identity {id} has {} heads at reformat time: {}", .heads.len(), .heads.join(", "))]
    MultipleHeads { id: String, heads: Vec<String> },
    #[error("identity {id} has no head at reformat time (cycle or orphan)")]
    NoHead { id: String },
}

#[derive(Debug, Clone)]
pub struct ReformatParams {
    pub author: String,
    pub provenance: String,
    pub ts: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReformattedKinograph {
    pub id: String,
    pub prior_version: String,
    pub new_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReformattedRoot {
    pub root_name: String,
    pub prior_version: String,
    pub new_version: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReformatReport {
    pub reformatted_kinographs: Vec<ReformattedKinograph>,
    pub skipped_kinographs_already_formatted: usize,
    pub reformatted_roots: Vec<ReformattedRoot>,
    pub skipped_roots_already_formatted: usize,
}

/// Walk the repo's root pointers and reachable kinograph kinos, rewriting
/// any remaining legacy-styx content into styxl.
///
/// Stages version events for regular kinograph kinos and writes root
/// pointers + store events for root kinographs. Caller is expected to
/// run a subsequent `kinora commit` to surface the staged kinograph
/// versions as heads for render.
pub fn reformat_repo(
    kinora_root: &Path,
    params: ReformatParams,
) -> Result<ReformatReport, ReformatError> {
    let cfg_text = fs::read_to_string(config_path(kinora_root))?;
    let config = Config::from_styx(&cfg_text)?;

    let store = ContentStore::new(kinora_root);
    let ledger = Ledger::new(kinora_root);

    let mut report = ReformatReport::default();

    // Hoist the ledger read out of Step 1's per-root loop so we pay the
    // scan cost once per reformat run. Step 2 reuses the same snapshot; if
    // Step 1 stages new root events the snapshot won't see them, but
    // Step 2 only uses the snapshot for kinograph identity lookups, not
    // root pointer traversal, so the staleness is benign.
    let events = ledger.read_all_events()?;

    // Step 1: reformat root kinographs in name order. Each root's pointer
    // is rewritten to the new blob + a store event recorded, mirroring
    // what `commit_root` does on a regular version bump.
    for root_name in config.roots.keys() {
        let Some(prior_hash) = read_root_pointer(kinora_root, root_name)? else {
            continue;
        };
        let content = store.read(&prior_hash)?;
        // Only take the styxl fast-path when the blob is valid UTF-8;
        // otherwise fall through to `RootKinograph::parse`, which surfaces
        // a proper error instead of silently counting the blob as
        // already-formatted.
        if let Ok(text) = std::str::from_utf8(&content)
            && is_styxl(text)
        {
            report.skipped_roots_already_formatted += 1;
            continue;
        }
        let root_kg = RootKinograph::parse(&content)?;
        let new_bytes = root_kg.to_styxl()?.into_bytes();
        if new_bytes == content {
            report.skipped_roots_already_formatted += 1;
            continue;
        }

        // Find the prior store event so we can carry its identity forward.
        let prior_event = events
            .iter()
            .find(|e| e.hash == prior_hash.as_hex() && e.is_store_event())
            .ok_or_else(|| ReformatError::PriorRootEventMissing {
                name: root_name.clone(),
                version: prior_hash.as_hex().to_owned(),
            })?
            .clone();

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
                parents: vec![prior_hash.as_hex().to_owned()],
            },
        )?;
        let new_hash = Hash::from_str(&stored.event.hash).map_err(|err| {
            ReformatError::InvalidHash {
                value: stored.event.hash.clone(),
                err,
            }
        })?;
        write_root_pointer(kinora_root, root_name, &new_hash)?;

        report.reformatted_roots.push(ReformattedRoot {
            root_name: root_name.clone(),
            prior_version: prior_hash.as_hex().to_owned(),
            new_version: new_hash.as_hex().to_owned(),
        });
    }

    // Step 2: walk every root pointer's current root kinograph, collect
    // kinograph-kind entry ids, and recurse into their heads' composition
    // entries. Reformat each kinograph kino we hit whose content is still
    // legacy-wrapped.
    let mut events_by_id = group_store_events_by_id(&events);

    // Synthesize store-event stubs from root kinograph entries whose
    // current heads have been archived out of staging (Never/MaxAge
    // drain). Without this, `pick_head` below would fail to resolve
    // drained heads. For entries already in staging we keep the staged
    // event untouched — the staged one is the newer head (reformat staged
    // a new version, or the user did).
    let mut to_visit: Vec<String> = Vec::new();
    for root_name in config.roots.keys() {
        let Some(hash) = read_root_pointer(kinora_root, root_name)? else {
            continue;
        };
        let content = store.read(&hash)?;
        let root_kg = RootKinograph::parse(&content)?;
        for entry in &root_kg.entries {
            if entry.kind == "kinograph" {
                to_visit.push(entry.id.clone());
            }
            if !events_by_id.contains_key(&entry.id) {
                let synth = Event::new_store(
                    entry.kind.clone(),
                    entry.id.clone(),
                    entry.version.clone(),
                    vec![],
                    entry.head_ts.clone(),
                    String::new(),
                    String::new(),
                    entry.metadata.clone(),
                );
                events_by_id.insert(entry.id.clone(), vec![synth]);
            }
        }
    }

    let mut visited: HashSet<String> = HashSet::new();
    while let Some(id) = to_visit.pop() {
        if !visited.insert(id.clone()) {
            continue;
        }
        let Some(group) = events_by_id.get(&id) else {
            continue;
        };
        let head = pick_head(&id, group)?;
        if head.kind != "kinograph" {
            continue;
        }

        let head_hash = Hash::from_str(&head.hash).map_err(|err| {
            ReformatError::InvalidHash {
                value: head.hash.clone(),
                err,
            }
        })?;
        let content = store.read(&head_hash)?;
        // Mirror the UTF-8 guard in Step 1: only fast-path when valid
        // UTF-8 + styxl-shaped; otherwise fall through to parse.
        if let Ok(text) = std::str::from_utf8(&content)
            && is_styxl(text)
        {
            report.skipped_kinographs_already_formatted += 1;
            continue;
        }
        let kg = Kinograph::parse(&content)?;
        let new_bytes = kg.to_styxl()?.into_bytes();
        if new_bytes == content {
            report.skipped_kinographs_already_formatted += 1;
            continue;
        }

        let stored = store_kino(
            kinora_root,
            StoreKinoParams {
                kind: "kinograph".into(),
                content: new_bytes,
                author: params.author.clone(),
                provenance: params.provenance.clone(),
                ts: params.ts.clone(),
                metadata: BTreeMap::new(),
                id: Some(head.id.clone()),
                parents: vec![head.hash.clone()],
            },
        )?;
        report.reformatted_kinographs.push(ReformattedKinograph {
            id: head.id.clone(),
            prior_version: head.hash.clone(),
            new_version: stored.event.hash.clone(),
        });

        // Recurse into composition entries so nested kinographs also get
        // reformatted in the same pass. When a nested entry's store
        // event is neither staged nor surfaced by any root kinograph
        // (archived-only, nested-only), `pick_head` on the next loop
        // iteration will find nothing in `events_by_id` and silently
        // skip. That's a pre-existing gap — reformat is best-effort on
        // post-archive graphs, and the next commit will pick up any
        // unreformatted bytes on a later pass.
        for entry in &kg.entries {
            if !visited.contains(&entry.id) {
                to_visit.push(entry.id.clone());
            }
        }
    }

    Ok(report)
}

fn group_store_events_by_id(events: &[Event]) -> BTreeMap<String, Vec<Event>> {
    let mut out: BTreeMap<String, Vec<Event>> = BTreeMap::new();
    for e in events {
        if e.is_store_event() {
            out.entry(e.id.clone()).or_default().push(e.clone());
        }
    }
    out
}

fn pick_head<'a>(id: &str, events: &'a [Event]) -> Result<&'a Event, ReformatError> {
    let referenced: HashSet<&str> = events
        .iter()
        .flat_map(|e| e.parents.iter().map(String::as_str))
        .collect();
    let heads: Vec<&Event> = events
        .iter()
        .filter(|e| !referenced.contains(e.hash.as_str()))
        .collect();
    match heads.as_slice() {
        [only] => Ok(*only),
        [] => Err(ReformatError::NoHead { id: id.to_owned() }),
        many => Err(ReformatError::MultipleHeads {
            id: id.to_owned(),
            heads: many.iter().map(|e| e.hash.clone()).collect(),
        }),
    }
}

fn write_root_pointer(kinora_root: &Path, root_name: &str, hash: &Hash) -> io::Result<()> {
    let dir = roots_dir(kinora_root);
    fs::create_dir_all(&dir)?;
    let path = root_pointer_path(kinora_root, root_name);
    let tmp = dir.join(format!(".{root_name}.tmp"));
    fs::write(&tmp, hash.as_hex())?;
    fs::rename(&tmp, &path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commit::{commit_all, commit_root, CommitParams};
    use crate::init::init;
    use crate::kinograph::Entry as KinographEntry;
    use crate::paths::kinora_root;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn setup() -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        init(tmp.path(), "https://example.com/x.git").unwrap();
        let root = kinora_root(tmp.path());
        (tmp, root)
    }

    fn reformat_params(ts: &str) -> ReformatParams {
        ReformatParams {
            author: "yj".into(),
            provenance: "reformat-test".into(),
            ts: ts.into(),
        }
    }

    fn commit_params(ts: &str) -> CommitParams {
        CommitParams {
            author: "yj".into(),
            provenance: "reformat-test".into(),
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
                provenance: "reformat-test".into(),
                ts: ts.into(),
                metadata: BTreeMap::from([("name".into(), name.into())]),
                id: None,
                parents: vec![],
            },
        )
        .unwrap();
        stored.event
    }

    /// Store a legacy-wrapped kinograph composing the given entry ids.
    fn store_legacy_kinograph(
        root: &Path,
        entry_ids: &[String],
        name: &str,
        ts: &str,
    ) -> Event {
        let entries: Vec<KinographEntry> = entry_ids
            .iter()
            .map(|id| KinographEntry::with_id(id.clone()))
            .collect();
        let k = Kinograph { entries };
        let content = k.to_styx().unwrap().into_bytes();
        assert!(
            !is_styxl(std::str::from_utf8(&content).unwrap()),
            "to_styx must emit legacy wrapped form for this test"
        );
        let stored = store_kino(
            root,
            StoreKinoParams {
                kind: "kinograph".into(),
                content,
                author: "yj".into(),
                provenance: "reformat-test".into(),
                ts: ts.into(),
                metadata: BTreeMap::from([("name".into(), name.into())]),
                id: None,
                parents: vec![],
            },
        )
        .unwrap();
        stored.event
    }

    /// Store a styxl-form kinograph composing the given entry ids.
    fn store_styxl_kinograph(
        root: &Path,
        entry_ids: &[String],
        name: &str,
        ts: &str,
    ) -> Event {
        let entries: Vec<KinographEntry> = entry_ids
            .iter()
            .map(|id| KinographEntry::with_id(id.clone()))
            .collect();
        let k = Kinograph { entries };
        let content = k.to_styxl().unwrap().into_bytes();
        assert!(
            is_styxl(std::str::from_utf8(&content).unwrap()),
            "to_styxl must emit styxl form for this test"
        );
        let stored = store_kino(
            root,
            StoreKinoParams {
                kind: "kinograph".into(),
                content,
                author: "yj".into(),
                provenance: "reformat-test".into(),
                ts: ts.into(),
                metadata: BTreeMap::from([("name".into(), name.into())]),
                id: None,
                parents: vec![],
            },
        )
        .unwrap();
        stored.event
    }

    /// Simulate a pre-migration root pointer: write a legacy-wrapped root
    /// blob directly via `store_kino(kind: "root")` and set the pointer to
    /// it. Returns the genesis root event.
    fn seed_legacy_root_pointer(
        root: &Path,
        root_name: &str,
        entries: Vec<crate::root::RootEntry>,
        ts: &str,
    ) -> Event {
        let rk = RootKinograph { entries };
        let content = rk.to_styx().unwrap().into_bytes();
        assert!(
            !is_styxl(std::str::from_utf8(&content).unwrap()),
            "RootKinograph::to_styx must emit legacy wrapped form for this test"
        );
        let stored = store_kino(
            root,
            StoreKinoParams {
                kind: "root".into(),
                content,
                author: "yj".into(),
                provenance: "reformat-test".into(),
                ts: ts.into(),
                metadata: BTreeMap::new(),
                id: None,
                parents: vec![],
            },
        )
        .unwrap();
        let hash = Hash::from_str(&stored.event.hash).unwrap();
        write_root_pointer(root, root_name, &hash).unwrap();
        stored.event
    }

    #[test]
    fn reformat_stages_new_version_for_legacy_kinograph_kino() {
        let (_t, root) = setup();
        let md = store_md(&root, b"hello", "hello", "2026-04-19T10:00:00Z");
        let kg_event =
            store_legacy_kinograph(&root, std::slice::from_ref(&md.id), "list", "2026-04-19T10:00:01Z");

        // Commit so the kinograph kino is reachable from the inbox root.
        commit_all(&root, commit_params("2026-04-19T10:00:02Z")).unwrap();

        let report =
            reformat_repo(&root, reformat_params("2026-04-19T10:00:03Z")).unwrap();
        assert_eq!(
            report.reformatted_kinographs.len(),
            1,
            "expected exactly one reformatted kinograph, got {report:#?}"
        );
        let entry = &report.reformatted_kinographs[0];
        assert_eq!(entry.id, kg_event.id, "identity carried forward");
        assert_eq!(entry.prior_version, kg_event.hash, "parent = current head");
        assert_ne!(entry.new_version, kg_event.hash, "new version must differ");

        let new_hash = Hash::from_str(&entry.new_version).unwrap();
        let new_bytes = ContentStore::new(&root).read(&new_hash).unwrap();
        let new_text = std::str::from_utf8(&new_bytes).unwrap();
        assert!(
            is_styxl(new_text),
            "new blob should be styxl form; got {new_text:?}",
        );

        let events = Ledger::new(&root).read_all_events().unwrap();
        let new_event = events
            .iter()
            .find(|e| e.hash == entry.new_version)
            .expect("new event must be in staged ledger");
        assert_eq!(new_event.id, kg_event.id);
        assert_eq!(new_event.parents, vec![kg_event.hash.clone()]);
        assert_eq!(new_event.kind, "kinograph");
    }

    #[test]
    fn reformat_is_idempotent_on_already_styxl_kinograph_kino() {
        let (_t, root) = setup();
        let md = store_md(&root, b"hello", "hello", "2026-04-19T10:00:00Z");
        store_styxl_kinograph(&root, std::slice::from_ref(&md.id), "list", "2026-04-19T10:00:01Z");
        commit_all(&root, commit_params("2026-04-19T10:00:02Z")).unwrap();

        let report =
            reformat_repo(&root, reformat_params("2026-04-19T10:00:03Z")).unwrap();
        assert!(report.reformatted_kinographs.is_empty());
        assert_eq!(report.skipped_kinographs_already_formatted, 1);
    }

    #[test]
    fn reformat_skips_markdown_and_text_kinos() {
        // Compose a legacy kinograph that references a markdown + a text
        // kino, so the graph walk actually visits the root entries. The
        // kinograph's own bytes get reformatted (covered by other tests);
        // we're asserting here that no new-version events are emitted for
        // the two opaque leaf kinos.
        let (_t, root) = setup();
        let md = store_md(&root, b"hello", "hello", "2026-04-19T10:00:00Z");
        let text_stored = store_kino(
            &root,
            StoreKinoParams {
                kind: "text".into(),
                content: b"plain text".to_vec(),
                author: "yj".into(),
                provenance: "reformat-test".into(),
                ts: "2026-04-19T10:00:01Z".into(),
                metadata: BTreeMap::from([("name".into(), "note".into())]),
                id: None,
                parents: vec![],
            },
        )
        .unwrap();
        let text_event = text_stored.event;
        store_legacy_kinograph(
            &root,
            &[md.id.clone(), text_event.id.clone()],
            "list",
            "2026-04-19T10:00:02Z",
        );
        commit_all(&root, commit_params("2026-04-19T10:00:03Z")).unwrap();

        let report =
            reformat_repo(&root, reformat_params("2026-04-19T10:00:04Z")).unwrap();
        // The composing kinograph is reformatted; markdown/text are never
        // visited as kinograph-kind entries, so neither skipped-counter
        // nor a new-version event should fire for them.
        assert_eq!(report.reformatted_kinographs.len(), 1);

        // Reformat must not stage any NEW version for md/text. Under
        // wcpp the originals are archived out of staging (owned by inbox,
        // drained on commit), so we count reformat-produced additions
        // rather than total versions visible in staging.
        let events_after = Ledger::new(&root).read_all_events().unwrap();
        let new_md: Vec<&Event> = events_after
            .iter()
            .filter(|e| e.id == md.id && e.is_store_event() && e.hash != md.hash)
            .collect();
        assert!(new_md.is_empty(), "markdown kino untouched");
        let new_text: Vec<&Event> = events_after
            .iter()
            .filter(|e| {
                e.id == text_event.id && e.is_store_event() && e.hash != text_event.hash
            })
            .collect();
        assert!(new_text.is_empty(), "text kino untouched");
    }

    #[test]
    fn reformat_rewrites_legacy_root_kinograph_and_updates_pointer() {
        let (_t, root) = setup();
        // Seed a legacy root pointer by hand (current commit code writes
        // styxl, so we can't produce a legacy root via commit_root).
        let md = store_md(&root, b"body", "body", "2026-04-19T10:00:00Z");
        let md_hash = Hash::from_str(&md.hash).unwrap();
        let entries = vec![crate::root::RootEntry::new(
            md.id.clone(),
            md_hash.as_hex(),
            "markdown",
            BTreeMap::from([("name".into(), "body".into())]),
            "",
        )];
        let prior_root = seed_legacy_root_pointer(
            &root,
            "inbox",
            entries,
            "2026-04-19T10:00:01Z",
        );

        let report =
            reformat_repo(&root, reformat_params("2026-04-19T10:00:02Z")).unwrap();
        assert_eq!(
            report.reformatted_roots.len(),
            1,
            "expected one reformatted root, got {report:#?}"
        );
        let reform = &report.reformatted_roots[0];
        assert_eq!(reform.root_name, "inbox");
        assert_eq!(reform.prior_version, prior_root.hash);
        assert_ne!(reform.new_version, prior_root.hash);

        // Pointer now points at the new blob.
        let pointer_body = fs::read_to_string(root_pointer_path(&root, "inbox")).unwrap();
        assert_eq!(pointer_body.trim(), reform.new_version);

        // New blob is styxl.
        let new_hash = Hash::from_str(&reform.new_version).unwrap();
        let new_bytes = ContentStore::new(&root).read(&new_hash).unwrap();
        assert!(is_styxl(std::str::from_utf8(&new_bytes).unwrap()));

        // A new root-kind store event is in the ledger, parented to the prior root.
        let events = Ledger::new(&root).read_all_events().unwrap();
        let new_event = events
            .iter()
            .find(|e| e.hash == reform.new_version)
            .expect("new root event should be staged");
        assert_eq!(new_event.kind, "root");
        assert_eq!(new_event.id, prior_root.id, "root identity carried forward");
        assert_eq!(new_event.parents, vec![prior_root.hash.clone()]);
    }

    #[test]
    fn reformat_is_idempotent_on_already_styxl_roots() {
        let (_t, root) = setup();
        store_md(&root, b"a", "a", "2026-04-19T10:00:00Z");
        // commit_all writes the root as styxl already.
        commit_all(&root, commit_params("2026-04-19T10:00:01Z")).unwrap();

        let pointer_before =
            fs::read_to_string(root_pointer_path(&root, "inbox")).unwrap();

        let report =
            reformat_repo(&root, reformat_params("2026-04-19T10:00:02Z")).unwrap();
        assert!(report.reformatted_roots.is_empty());
        assert!(
            report.skipped_roots_already_formatted >= 1,
            "expected at least the inbox root to be counted as already-formatted",
        );

        let pointer_after =
            fs::read_to_string(root_pointer_path(&root, "inbox")).unwrap();
        assert_eq!(
            pointer_before, pointer_after,
            "pointer should not change on an already-styxl repo"
        );
    }

    #[test]
    fn reformat_recurses_into_nested_composition_entries() {
        let (_t, root) = setup();
        let leaf = store_md(&root, b"leaf", "leaf", "2026-04-19T10:00:00Z");
        // Inner legacy kinograph pointing at `leaf`.
        let inner = store_legacy_kinograph(
            &root,
            std::slice::from_ref(&leaf.id),
            "inner",
            "2026-04-19T10:00:01Z",
        );
        // Outer legacy kinograph composing `inner` — reformat must walk
        // into `inner` via the outer's composition entries.
        let outer = store_legacy_kinograph(
            &root,
            std::slice::from_ref(&inner.id),
            "outer",
            "2026-04-19T10:00:02Z",
        );
        commit_all(&root, commit_params("2026-04-19T10:00:03Z")).unwrap();

        let report =
            reformat_repo(&root, reformat_params("2026-04-19T10:00:04Z")).unwrap();
        let mut ids: Vec<&str> = report
            .reformatted_kinographs
            .iter()
            .map(|e| e.id.as_str())
            .collect();
        ids.sort();
        let mut expected = vec![inner.id.as_str(), outer.id.as_str()];
        expected.sort();
        assert_eq!(
            ids, expected,
            "both outer and inner kinographs should have been reformatted",
        );
    }

    /// Store a legacy-wrapped kinograph with a single composition entry
    /// carrying a `pin = version` hint. Used to construct the nested-only
    /// scenario where reformat must resolve the inner kinograph via its
    /// pin after the inner's store event has been drained from staging.
    fn store_legacy_kinograph_pinned(
        root: &Path,
        entry_id: &str,
        entry_pin: &str,
        name: &str,
        ts: &str,
    ) -> Event {
        let k = Kinograph {
            entries: vec![KinographEntry {
                id: entry_id.to_owned(),
                name: String::new(),
                pin: entry_pin.to_owned(),
                note: String::new(),
            }],
        };
        let content = k.to_styx().unwrap().into_bytes();
        assert!(
            !is_styxl(std::str::from_utf8(&content).unwrap()),
            "to_styx must emit legacy wrapped form for this test"
        );
        let stored = store_kino(
            root,
            StoreKinoParams {
                kind: "kinograph".into(),
                content,
                author: "yj".into(),
                provenance: "reformat-test".into(),
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
    fn reformat_resolves_nested_pin_when_store_event_drained() {
        use crate::paths::staged_event_path;

        let (_t, root) = setup();

        // Leaf markdown + inner legacy kinograph composing leaf.
        let leaf = store_md(&root, b"leaf", "leaf", "2026-04-21T10:00:00Z");
        let inner = store_legacy_kinograph(
            &root,
            std::slice::from_ref(&leaf.id),
            "inner",
            "2026-04-21T10:00:01Z",
        );

        // Outer legacy kinograph composing inner WITH pin=inner.hash.
        // The pin is the hint reformat must use to resolve inner after
        // its store event is drained from staging.
        let outer = store_legacy_kinograph_pinned(
            &root,
            &inner.id,
            &inner.hash,
            "outer",
            "2026-04-21T10:00:02Z",
        );

        // Seed a legacy root pointer containing ONLY outer — simulating
        // a state where inner is not surfaced in any root kinograph.
        let outer_hash = Hash::from_str(&outer.hash).unwrap();
        let entries = vec![crate::root::RootEntry::new(
            outer.id.clone(),
            outer_hash.as_hex(),
            "kinograph",
            BTreeMap::from([("name".into(), "outer".into())]),
            "2026-04-21T10:00:02Z",
        )];
        seed_legacy_root_pointer(&root, "inbox", entries, "2026-04-21T10:00:03Z");

        // Simulate wcpp drain: remove inner's + leaf's staged events so
        // `events_by_id` won't carry them. Outer's staged event stays
        // so the root's synth path still works for the top-level entry.
        let inner_event_hash = inner.event_hash().unwrap();
        let leaf_event_hash = leaf.event_hash().unwrap();
        fs::remove_file(staged_event_path(&root, &inner_event_hash)).unwrap();
        fs::remove_file(staged_event_path(&root, &leaf_event_hash)).unwrap();

        let report =
            reformat_repo(&root, reformat_params("2026-04-21T10:00:04Z")).unwrap();

        let mut ids: Vec<&str> = report
            .reformatted_kinographs
            .iter()
            .map(|e| e.id.as_str())
            .collect();
        ids.sort();
        let mut expected = vec![inner.id.as_str(), outer.id.as_str()];
        expected.sort();
        assert_eq!(
            ids, expected,
            "both outer and inner should reformat; inner needs synthesis from its pin; got {report:#?}"
        );

        // Each reformatted kinograph's new blob is in styxl form.
        for reformatted in &report.reformatted_kinographs {
            let new_hash = Hash::from_str(&reformatted.new_version).unwrap();
            let new_bytes = ContentStore::new(&root).read(&new_hash).unwrap();
            assert!(
                is_styxl(std::str::from_utf8(&new_bytes).unwrap()),
                "new blob for {} should be styxl; got {:?}",
                reformatted.id,
                new_bytes,
            );
        }
    }

    #[test]
    fn reformat_then_commit_makes_new_version_the_head() {
        let (_t, root) = setup();
        let md = store_md(&root, b"hello", "hello", "2026-04-19T10:00:00Z");
        let kg_event =
            store_legacy_kinograph(&root, std::slice::from_ref(&md.id), "list", "2026-04-19T10:00:01Z");
        commit_all(&root, commit_params("2026-04-19T10:00:02Z")).unwrap();

        let _report =
            reformat_repo(&root, reformat_params("2026-04-19T10:00:03Z")).unwrap();
        // A subsequent commit should promote the reformatted version and
        // leave the root pointing at a new root-blob whose entry for the
        // kinograph lists the new version hash.
        let commit = commit_root(&root, "inbox", commit_params("2026-04-19T10:00:04Z"))
            .unwrap();
        let new_root_hash = commit.new_version.expect("inbox should advance");
        let new_root_bytes = ContentStore::new(&root).read(&new_root_hash).unwrap();
        let rk = RootKinograph::parse(&new_root_bytes).unwrap();
        let kg_entry = rk
            .entries
            .iter()
            .find(|e| e.id == kg_event.id)
            .expect("kinograph entry must be in the new root");
        assert_ne!(
            kg_entry.version, kg_event.hash,
            "post-reformat commit should bump the entry's version away from the legacy blob",
        );
    }
}
