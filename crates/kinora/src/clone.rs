//! Rebuild a `.kinora/` directory into a fresh target dir: copy reachable
//! blobs through the current store API, rewriting legacy extensionless
//! filenames into the canonical `<hash>.<ext>` form and dropping blobs
//! that nothing points at.
//!
//! Clone is **hash-preserving** — content bytes are never rewritten. For
//! on-blob content migration (e.g. legacy styx → styxl) use
//! `kinora::reformat`.
//!
//! Both arguments are direct paths to `.kinora/` dirs — clone does not
//! walk up looking for a repo root.
//!
//! Reachability walk:
//!
//! 1. Every declared root's pointer → root-blob hash reachable.
//! 2. Parse the root blob; each `RootEntry`'s `version` hash reachable.
//! 3. For entries of kind `kinograph`, recurse via staged events' head
//!    (same `pick_head` rule as `reformat`) into composition-entry ids.
//!
//! Staged events are copied selectively: store events whose blob hash is
//! reachable, plus every non-store event (assigns carry routing state
//! that the dst resolver would otherwise lack).

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::commit::{read_root_pointer, CommitError};
use crate::config::{Config, ConfigError};
use crate::event::{Event, EventError};
use crate::hash::{Hash, HashParseError};
use crate::kinograph::{Kinograph, KinographError};
use crate::ledger::{Ledger, LedgerError};
use crate::namespace::ext_for_kind;
use crate::paths::{
    config_path, find_blob_path, ledger_dir, root_pointer_path, roots_dir, staged_dir, store_dir,
};
use crate::root::{RootError, RootKinograph};
use crate::store::{ContentStore, StoreError};

#[derive(Debug, thiserror::Error)]
pub enum CloneError {
    #[error("clone io error: {0}")]
    Io(#[from] io::Error),
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Ledger(#[from] LedgerError),
    #[error(transparent)]
    Store(#[from] StoreError),
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
    #[error("src is not a kinora directory (missing config.styx): {}", .path.display())]
    SrcInvalid { path: PathBuf },
    #[error("dst is not empty: {}", .path.display())]
    DstNotEmpty { path: PathBuf },
    #[error("identity {id} has {} heads in src: {}", .heads.len(), .heads.join(", "))]
    MultipleHeads { id: String, heads: Vec<String> },
    #[error("identity {id} has no head in src (cycle or orphan)")]
    NoHead { id: String },
}

/// Caller-supplied provenance for the clone operation. Not yet stamped
/// into any on-wire artifact, but keeps the signature symmetric with
/// other library entry points (store, commit, reformat) so a future
/// archive-of-clone-run can land without an API break.
#[derive(Debug, Clone)]
pub struct CloneParams {
    pub author: String,
    pub provenance: String,
    pub ts: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CloneReport {
    /// Number of unique reachable blobs copied into dst.
    pub kinos_rebuilt: usize,
    /// Number of src blobs that were not reachable and therefore omitted.
    pub blobs_dropped: usize,
    /// Number of blobs whose canonical dst filename differs from the
    /// stem-matching filename in src (e.g. legacy extensionless blobs
    /// written before kinora-wpup getting a `<hash>.<ext>` name in dst).
    pub filenames_rewritten: usize,
}

/// Rebuild `src` into `dst`.
pub fn clone_repo(
    src: &Path,
    dst: &Path,
    _params: CloneParams,
) -> Result<CloneReport, CloneError> {
    let src_cfg_path = config_path(src);
    if !src_cfg_path.is_file() {
        return Err(CloneError::SrcInvalid { path: src.to_path_buf() });
    }
    let cfg_text = fs::read_to_string(&src_cfg_path)?;
    let config = Config::from_styx(&cfg_text)?;

    ensure_dst_empty(dst)?;
    fs::create_dir_all(dst)?;
    fs::write(config_path(dst), &cfg_text)?;
    fs::create_dir_all(store_dir(dst))?;
    fs::create_dir_all(ledger_dir(dst))?;
    fs::create_dir_all(staged_dir(dst))?;
    fs::create_dir_all(roots_dir(dst))?;

    let src_store = ContentStore::new(src);
    let dst_store = ContentStore::new(dst);
    let src_ledger = Ledger::new(src);
    let events = src_ledger.read_all_events()?;
    let events_by_id = group_store_events_by_id(&events);

    // reachable_blobs: set of reachable blob hashes (hex).
    // reachable_kinds: first-seen kind for each reachable hash — chosen
    //   so ContentStore::write emits the correct dst extension. Root
    //   entries carry the authoritative kind, so we prefer them; falls
    //   back to the ledger-event kind for composition walks.
    let mut reachable_blobs: HashSet<String> = HashSet::new();
    let mut reachable_kinds: BTreeMap<String, String> = BTreeMap::new();

    let mut id_queue: Vec<String> = Vec::new();
    let mut visited_ids: HashSet<String> = HashSet::new();

    for root_name in config.roots.keys() {
        let Some(root_hash) = read_root_pointer(src, root_name)? else {
            continue;
        };
        let hex = root_hash.as_hex().to_owned();
        reachable_blobs.insert(hex.clone());
        reachable_kinds.insert(hex, "root".to_owned());
        let content = src_store.read(&root_hash)?;
        let rk = RootKinograph::parse(&content)?;
        for entry in &rk.entries {
            let version = Hash::from_str(&entry.version).map_err(|err| {
                CloneError::InvalidHash {
                    value: entry.version.clone(),
                    err,
                }
            })?;
            let vhex = version.as_hex().to_owned();
            reachable_blobs.insert(vhex.clone());
            reachable_kinds
                .entry(vhex)
                .or_insert_with(|| entry.kind.clone());
            if entry.kind == "kinograph" {
                id_queue.push(entry.id.clone());
            }
        }
    }

    while let Some(id) = id_queue.pop() {
        if !visited_ids.insert(id.clone()) {
            continue;
        }
        let Some(group) = events_by_id.get(&id) else {
            continue;
        };
        let head = pick_head(&id, group)?;
        let head_hash = Hash::from_str(&head.hash).map_err(|err| {
            CloneError::InvalidHash {
                value: head.hash.clone(),
                err,
            }
        })?;
        let hex = head_hash.as_hex().to_owned();
        reachable_blobs.insert(hex.clone());
        reachable_kinds
            .entry(hex)
            .or_insert_with(|| head.kind.clone());
        if head.kind != "kinograph" {
            continue;
        }
        let content = src_store.read(&head_hash)?;
        let kg = Kinograph::parse(&content)?;
        for entry in &kg.entries {
            if !visited_ids.contains(&entry.id) {
                id_queue.push(entry.id.clone());
            }
        }
    }

    let src_blobs = enumerate_blobs(src)?;
    let mut report = CloneReport::default();

    for hex in &reachable_blobs {
        let hash = Hash::from_str(hex).map_err(|err| CloneError::InvalidHash {
            value: hex.clone(),
            err,
        })?;
        let content = src_store.read(&hash)?;
        let kind = reachable_kinds
            .get(hex)
            .map(String::as_str)
            .unwrap_or("binary");
        let src_path = find_blob_path(src, &hash).ok_or_else(|| {
            CloneError::Io(io::Error::new(
                io::ErrorKind::NotFound,
                format!("reachable blob vanished from src: {hex}"),
            ))
        })?;
        let src_name = src_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        let canonical_name = match ext_for_kind(kind) {
            Some(e) => format!("{}.{e}", hash.as_hex()),
            None => hash.as_hex().to_owned(),
        };
        if src_name != canonical_name {
            report.filenames_rewritten += 1;
        }
        dst_store.write(kind, &content)?;
        report.kinos_rebuilt += 1;
    }

    report.blobs_dropped = src_blobs.len().saturating_sub(reachable_blobs.len());

    for root_name in config.roots.keys() {
        if let Some(hash) = read_root_pointer(src, root_name)? {
            write_root_pointer(dst, root_name, &hash)?;
        }
    }

    let dst_ledger = Ledger::new(dst);
    for event in &events {
        let keep = if event.is_store_event() {
            reachable_blobs.contains(&event.hash)
        } else {
            true
        };
        if !keep {
            continue;
        }
        dst_ledger.write_event(event)?;
    }

    Ok(report)
}

fn ensure_dst_empty(dst: &Path) -> Result<(), CloneError> {
    match fs::read_dir(dst) {
        Ok(mut iter) => {
            if iter.next().is_some() {
                return Err(CloneError::DstNotEmpty { path: dst.to_path_buf() });
            }
            Ok(())
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(CloneError::Io(e)),
    }
}

/// Enumerate every real blob under `<src>/store/` by stem — 64-hex files,
/// extension optional. Tmp files (`.tmp-*`) and anything else is skipped.
fn enumerate_blobs(src_kinora: &Path) -> Result<HashSet<String>, CloneError> {
    let mut out = HashSet::new();
    let store = store_dir(src_kinora);
    if !store.is_dir() {
        return Ok(out);
    }
    for shard in fs::read_dir(&store)? {
        let shard = shard?;
        if !shard.file_type()?.is_dir() {
            continue;
        }
        for entry in fs::read_dir(shard.path())? {
            let path = entry?.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with(".tmp-") || name.starts_with('.') {
                continue;
            }
            let stem = name.split_once('.').map(|(l, _)| l).unwrap_or(name);
            if stem.len() == 64 && stem.bytes().all(|b| b.is_ascii_hexdigit()) {
                out.insert(stem.to_owned());
            }
        }
    }
    Ok(out)
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

fn pick_head<'a>(id: &str, events: &'a [Event]) -> Result<&'a Event, CloneError> {
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
        [] => Err(CloneError::NoHead { id: id.to_owned() }),
        many => Err(CloneError::MultipleHeads {
            id: id.to_owned(),
            heads: many.iter().map(|e| e.hash.clone()).collect(),
        }),
    }
}

fn write_root_pointer(
    kinora_root: &Path,
    root_name: &str,
    hash: &Hash,
) -> Result<(), CloneError> {
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
    use crate::assign::{write_assign, AssignEvent};
    use crate::commit::{commit_all, CommitParams};
    use crate::init::init;
    use crate::kino::{store_kino, StoreKinoParams};
    use crate::kinograph::Entry as KinographEntry;
    use crate::paths::kinora_root;
    use std::collections::BTreeMap;
    use tempfile::TempDir;

    fn setup() -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        init(tmp.path(), "https://example.com/x.git").unwrap();
        let k = kinora_root(tmp.path());
        (tmp, k)
    }

    fn empty_dst() -> TempDir {
        TempDir::new().unwrap()
    }

    fn clone_params(ts: &str) -> CloneParams {
        CloneParams {
            author: "yj".into(),
            provenance: "clone-test".into(),
            ts: ts.into(),
        }
    }

    fn commit_params(ts: &str) -> CommitParams {
        CommitParams {
            author: "yj".into(),
            provenance: "clone-test".into(),
            ts: ts.into(),
        }
    }

    fn store_md(root: &Path, body: &[u8], name: &str, ts: &str) -> Event {
        store_kino(
            root,
            StoreKinoParams {
                kind: "markdown".into(),
                content: body.to_vec(),
                author: "yj".into(),
                provenance: "clone-test".into(),
                ts: ts.into(),
                metadata: BTreeMap::from([("name".into(), name.into())]),
                id: None,
                parents: vec![],
            },
        )
        .unwrap()
        .event
    }

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
        store_kino(
            root,
            StoreKinoParams {
                kind: "kinograph".into(),
                content,
                author: "yj".into(),
                provenance: "clone-test".into(),
                ts: ts.into(),
                metadata: BTreeMap::from([("name".into(), name.into())]),
                id: None,
                parents: vec![],
            },
        )
        .unwrap()
        .event
    }

    #[test]
    fn clone_errors_when_src_has_no_config() {
        let src = TempDir::new().unwrap(); // just an empty dir, not a kinora dir
        let dst = empty_dst();
        let err = clone_repo(
            src.path(),
            &dst.path().join("dst"),
            clone_params("2026-04-20T09:00:00Z"),
        )
        .unwrap_err();
        assert!(matches!(err, CloneError::SrcInvalid { .. }), "got: {err:?}");
    }

    #[test]
    fn clone_errors_when_dst_is_nonempty() {
        let (_t, src) = setup();
        let dst_tmp = TempDir::new().unwrap();
        let dst = dst_tmp.path().join("dst");
        fs::create_dir_all(&dst).unwrap();
        fs::write(dst.join("stray"), b"hi").unwrap();
        let err = clone_repo(&src, &dst, clone_params("2026-04-20T09:00:00Z"))
            .unwrap_err();
        assert!(matches!(err, CloneError::DstNotEmpty { .. }), "got: {err:?}");
    }

    #[test]
    fn clone_empty_repo_copies_config_and_creates_layout() {
        let (_t, src) = setup();
        let dst_tmp = TempDir::new().unwrap();
        let dst = dst_tmp.path().join("dst");

        let report = clone_repo(&src, &dst, clone_params("2026-04-20T09:00:00Z"))
            .unwrap();
        assert_eq!(report.kinos_rebuilt, 0);
        assert_eq!(report.blobs_dropped, 0);
        assert_eq!(report.filenames_rewritten, 0);

        assert!(config_path(&dst).is_file());
        assert!(store_dir(&dst).is_dir());
        assert!(staged_dir(&dst).is_dir());
        assert!(roots_dir(&dst).is_dir());
        assert!(ledger_dir(&dst).is_dir());

        // config.styx copied verbatim
        let src_cfg = fs::read(config_path(&src)).unwrap();
        let dst_cfg = fs::read(config_path(&dst)).unwrap();
        assert_eq!(src_cfg, dst_cfg);
    }

    #[test]
    fn clone_empty_repo_creates_missing_dst_dir() {
        let (_t, src) = setup();
        let dst_tmp = TempDir::new().unwrap();
        // dst is a path inside dst_tmp that doesn't exist yet.
        let dst = dst_tmp.path().join("fresh_clone");
        assert!(!dst.exists());
        clone_repo(&src, &dst, clone_params("2026-04-20T09:00:00Z")).unwrap();
        assert!(dst.is_dir());
        assert!(config_path(&dst).is_file());
    }

    #[test]
    fn clone_copies_reachable_kino_and_its_root_after_commit() {
        let (_t, src) = setup();
        let md = store_md(&src, b"hello", "hello", "2026-04-20T09:00:00Z");
        commit_all(&src, commit_params("2026-04-20T09:00:01Z")).unwrap();

        let dst_tmp = TempDir::new().unwrap();
        let dst = dst_tmp.path().join("dst");
        let report = clone_repo(&src, &dst, clone_params("2026-04-20T09:00:02Z"))
            .unwrap();

        // Reachable: the markdown blob, the inbox root blob, and the
        // commits-root-related archive + commits root blob. Exact count
        // varies with commit_all, but the markdown must be there.
        let md_hash = Hash::from_str(&md.hash).unwrap();
        assert!(
            ContentStore::new(&dst).exists(&md_hash),
            "markdown blob missing in dst"
        );

        // inbox root pointer copied
        let src_ptr = read_root_pointer(&src, "inbox").unwrap().unwrap();
        let dst_ptr = read_root_pointer(&dst, "inbox").unwrap().unwrap();
        assert_eq!(src_ptr.as_hex(), dst_ptr.as_hex());
        assert!(
            ContentStore::new(&dst).exists(&src_ptr),
            "inbox root blob not in dst store"
        );

        assert!(report.kinos_rebuilt >= 2, "expected at least 2 rebuilt, got {report:?}");
    }

    #[test]
    fn clone_drops_unreachable_blob() {
        let (_t, src) = setup();
        // Reachable: one markdown, committed so it's entered in inbox.
        let reachable = store_md(&src, b"reachable", "reachable", "2026-04-20T09:00:00Z");
        commit_all(&src, commit_params("2026-04-20T09:00:01Z")).unwrap();

        // Unreachable: assign to a root, then supersede with another
        // assign targeting the same kino to a different root — neither
        // commits this blob because we don't commit after. Actually a
        // simpler path: just store and never commit.
        let unreachable = store_md(&src, b"orphan", "orphan", "2026-04-20T09:00:02Z");
        // The `orphan` kino's store event is staged, but since we don't
        // call commit_all after, it never ends up in any root. Its blob
        // is present in the store but unreachable through root pointers.

        let dst_tmp = TempDir::new().unwrap();
        let dst = dst_tmp.path().join("dst");
        let report = clone_repo(&src, &dst, clone_params("2026-04-20T09:00:03Z"))
            .unwrap();

        let reach_hash = Hash::from_str(&reachable.hash).unwrap();
        let unreach_hash = Hash::from_str(&unreachable.hash).unwrap();
        assert!(ContentStore::new(&dst).exists(&reach_hash));
        assert!(
            !ContentStore::new(&dst).exists(&unreach_hash),
            "unreachable blob must be dropped"
        );
        assert!(
            report.blobs_dropped >= 1,
            "blobs_dropped should count the orphan: {report:?}"
        );
    }

    #[test]
    fn clone_rebuilds_composition_kinograph_recursively() {
        let (_t, src) = setup();
        let leaf = store_md(&src, b"leaf", "leaf", "2026-04-20T09:00:00Z");
        let inner = store_styxl_kinograph(
            &src,
            std::slice::from_ref(&leaf.id),
            "inner",
            "2026-04-20T09:00:01Z",
        );
        let outer = store_styxl_kinograph(
            &src,
            std::slice::from_ref(&inner.id),
            "outer",
            "2026-04-20T09:00:02Z",
        );
        commit_all(&src, commit_params("2026-04-20T09:00:03Z")).unwrap();

        let dst_tmp = TempDir::new().unwrap();
        let dst = dst_tmp.path().join("dst");
        clone_repo(&src, &dst, clone_params("2026-04-20T09:00:04Z")).unwrap();

        for kino in [&leaf, &inner, &outer] {
            let h = Hash::from_str(&kino.hash).unwrap();
            assert!(
                ContentStore::new(&dst).exists(&h),
                "expected {} in dst store",
                kino.hash,
            );
        }
    }

    #[test]
    fn clone_rewrites_legacy_extensionless_blob_to_canonical_name() {
        let (_t, src) = setup();
        let md = store_md(&src, b"legacy", "legacy", "2026-04-20T09:00:00Z");
        commit_all(&src, commit_params("2026-04-20T09:00:01Z")).unwrap();

        // Hand-rewrite the markdown blob in src to the legacy (no-ext)
        // filename to simulate a pre-wpup repo.
        let md_hash = Hash::from_str(&md.hash).unwrap();
        let canonical = find_blob_path(&src, &md_hash).unwrap();
        let shard = canonical.parent().unwrap().to_path_buf();
        let legacy = shard.join(md_hash.as_hex());
        fs::rename(&canonical, &legacy).unwrap();

        let dst_tmp = TempDir::new().unwrap();
        let dst = dst_tmp.path().join("dst");
        let report = clone_repo(&src, &dst, clone_params("2026-04-20T09:00:02Z"))
            .unwrap();
        assert!(
            report.filenames_rewritten >= 1,
            "expected at least one filename rewrite: {report:?}",
        );

        // dst must hold the blob under the canonical `<hash>.md` name.
        let dst_blob = find_blob_path(&dst, &md_hash).unwrap();
        assert_eq!(
            dst_blob.file_name().and_then(|n| n.to_str()).unwrap(),
            format!("{}.md", md_hash.as_hex()),
            "dst blob should use canonical extension",
        );
    }

    #[test]
    fn clone_surfaces_hash_mismatch_on_corrupt_reachable_blob() {
        let (_t, src) = setup();
        let md = store_md(&src, b"authentic", "auth", "2026-04-20T09:00:00Z");
        commit_all(&src, commit_params("2026-04-20T09:00:01Z")).unwrap();

        // Corrupt the reachable markdown blob.
        let h = Hash::from_str(&md.hash).unwrap();
        let p = find_blob_path(&src, &h).unwrap();
        fs::write(&p, b"tampered").unwrap();

        let dst_tmp = TempDir::new().unwrap();
        let dst = dst_tmp.path().join("dst");
        let err = clone_repo(&src, &dst, clone_params("2026-04-20T09:00:02Z"))
            .unwrap_err();
        assert!(
            matches!(err, CloneError::Store(StoreError::HashMismatch { .. })),
            "got: {err:?}"
        );
    }

    #[test]
    fn clone_preserves_assign_events_in_staged() {
        // Assigns aren't store events — they carry routing state. Clone
        // must copy them into dst/staged verbatim so dst's resolver/
        // commit sees the same routing picture.
        let (_t, src) = setup();
        // Declare a second root so an assign to it is valid. keep-last-10
        // (not Never) so the assign survives in staging through commit_all
        // — kinora-bayr's Never prune would drop it into the archive blob,
        // which is orthogonal to what this test covers (clone mechanics).
        let cfg_text = fs::read_to_string(config_path(&src)).unwrap();
        let mut cfg = Config::from_styx(&cfg_text).unwrap();
        cfg.roots.insert(
            "main".into(),
            crate::config::RootPolicy::KeepLastN(10),
        );
        fs::write(config_path(&src), cfg.to_styx().unwrap()).unwrap();

        let md = store_md(&src, b"hello", "hello", "2026-04-20T09:00:00Z");
        let assign = AssignEvent {
            kino_id: md.id.clone(),
            target_root: "main".into(),
            supersedes: vec![],
            author: "yj".into(),
            ts: "2026-04-20T09:00:01Z".into(),
            provenance: "clone-test".into(),
        };
        write_assign(&src, &assign).unwrap();
        commit_all(&src, commit_params("2026-04-20T09:00:02Z")).unwrap();

        let dst_tmp = TempDir::new().unwrap();
        let dst = dst_tmp.path().join("dst");
        clone_repo(&src, &dst, clone_params("2026-04-20T09:00:03Z")).unwrap();

        // Find the assign event in dst's staged tree.
        let dst_events = Ledger::new(&dst).read_all_events().unwrap();
        let has_assign = dst_events
            .iter()
            .any(|e| !e.is_store_event() && e.id == md.id);
        assert!(
            has_assign,
            "expected the assign event to survive into dst/staged; got {} events",
            dst_events.len(),
        );
    }

    #[test]
    fn clone_dst_resolver_matches_src_resolver_for_reachable_kino() {
        // End-to-end: after clone, loading a resolver on dst should
        // produce the same content bytes for a committed kino as src.
        let (_t, src) = setup();
        let md = store_md(&src, b"resolvable", "resolvable", "2026-04-20T09:00:00Z");
        commit_all(&src, commit_params("2026-04-20T09:00:01Z")).unwrap();

        let dst_tmp = TempDir::new().unwrap();
        let dst = dst_tmp.path().join("dst");
        clone_repo(&src, &dst, clone_params("2026-04-20T09:00:02Z")).unwrap();

        let src_resolver = crate::resolve::Resolver::load(&src).unwrap();
        let dst_resolver = crate::resolve::Resolver::load(&dst).unwrap();
        let src_bytes = src_resolver.resolve_by_id(&md.id).unwrap().content;
        let dst_bytes = dst_resolver.resolve_by_id(&md.id).unwrap().content;
        assert_eq!(src_bytes, dst_bytes);
    }

    #[test]
    fn clone_reports_zero_dropped_when_everything_is_reachable() {
        // Symmetric counterpart to clone_drops_unreachable_blob — if
        // every src blob is committed, the report should show zero
        // dropped.
        let (_t, src) = setup();
        store_md(&src, b"a", "a", "2026-04-20T09:00:00Z");
        store_md(&src, b"b", "b", "2026-04-20T09:00:01Z");
        commit_all(&src, commit_params("2026-04-20T09:00:02Z")).unwrap();

        let dst_tmp = TempDir::new().unwrap();
        let dst = dst_tmp.path().join("dst");
        let report = clone_repo(&src, &dst, clone_params("2026-04-20T09:00:03Z"))
            .unwrap();
        assert_eq!(report.blobs_dropped, 0, "nothing should be dropped: {report:?}");
    }

    #[test]
    fn clone_leaves_src_unchanged() {
        let (_t, src) = setup();
        let _md = store_md(&src, b"keep", "keep", "2026-04-20T09:00:00Z");
        commit_all(&src, commit_params("2026-04-20T09:00:01Z")).unwrap();

        let before: BTreeMap<PathBuf, Vec<u8>> = walk_files(&src);

        let dst_tmp = TempDir::new().unwrap();
        let dst = dst_tmp.path().join("dst");
        clone_repo(&src, &dst, clone_params("2026-04-20T09:00:02Z")).unwrap();

        let after: BTreeMap<PathBuf, Vec<u8>> = walk_files(&src);
        assert_eq!(before, after, "src must be byte-identical after clone");
    }

    /// Recursively gather every file under `root` keyed by relative path,
    /// so tests can assert a directory is untouched across an operation.
    fn walk_files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        fn visit(root: &Path, cur: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for entry in fs::read_dir(cur).unwrap() {
                let entry = entry.unwrap();
                let p = entry.path();
                if entry.file_type().unwrap().is_dir() {
                    visit(root, &p, out);
                } else {
                    let rel = p.strip_prefix(root).unwrap().to_path_buf();
                    out.insert(rel, fs::read(&p).unwrap());
                }
            }
        }
        let mut out = BTreeMap::new();
        visit(root, root, &mut out);
        out
    }
}
