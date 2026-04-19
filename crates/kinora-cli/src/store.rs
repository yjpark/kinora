use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read};
use std::path::Path;

use kinora::assign::{write_assign, AssignEvent};
use kinora::author::resolve_author_from_git;
use kinora::kino::{store_kino, StoreKinoParams, StoredKino};
use kinora::kinograph::Kinograph;
use kinora::paths::{staged_event_path, kinora_root};
use kinora::resolve::Resolver;

use crate::common::{find_repo_root, parse_metadata_flag, parse_parents, CliError};

/// Inputs to the `store` subcommand — mirrors the figue-parsed fields so
/// the runner is pure (no argv, no env) and easy to unit-test.
pub struct StoreRunArgs {
    pub kind: String,
    pub path: Option<String>,
    pub provenance: String,
    pub name: Option<String>,
    pub id: Option<String>,
    pub parents: Option<String>,
    pub draft: bool,
    pub author: Option<String>,
    pub metadata: Vec<String>,
    pub root: Option<String>,
}

pub fn run_store(cwd: &Path, args: StoreRunArgs) -> Result<StoredKino, CliError> {
    let repo_root = find_repo_root(cwd)?;
    let kin_root = kinora_root(&repo_root);

    if let Some(root) = args.root.as_deref()
        && root.is_empty()
    {
        return Err(CliError::EmptyRoot);
    }

    let raw_content = read_content(args.path.as_deref())?;
    let content = if args.kind == "kinograph" {
        normalize_kinograph_content(&kin_root, &raw_content)?
    } else {
        raw_content
    };

    let mut metadata: BTreeMap<String, String> = BTreeMap::new();
    if let Some(name) = args.name {
        metadata.insert("name".into(), name);
    }
    for kv in &args.metadata {
        let (k, v) = parse_metadata_flag(kv)?;
        if k == "draft" && args.draft {
            return Err(CliError::ConflictingDraftFlag);
        }
        metadata.insert(k, v);
    }
    if args.draft {
        metadata.insert("draft".into(), "true".into());
    }

    let parents = parse_parents(args.parents.as_deref());

    let author = match args.author {
        Some(a) => a,
        None => resolve_author_from_git(&repo_root).ok_or(CliError::AuthorUnresolved)?,
    };

    let ts = jiff::Timestamp::now().to_string();

    let params = StoreKinoParams {
        kind: args.kind,
        content,
        author: author.clone(),
        provenance: args.provenance.clone(),
        ts: ts.clone(),
        metadata,
        id: args.id,
        parents,
    };
    let stored = store_kino(&kin_root, params)?;

    if let Some(root) = args.root {
        let assign = AssignEvent {
            kino_id: stored.event.id.clone(),
            target_root: root,
            supersedes: vec![],
            author,
            ts,
            provenance: args.provenance,
        };
        pair_assign_with_rollback(&kin_root, &stored, &assign)?;
    }

    Ok(stored)
}

/// Write `assign` as the second half of a `kinora store --root` pair.
/// On failure, best-effort deletes the store event's staged file iff this
/// call introduced it (stored.was_new_lineage), preserving the atomic-pair
/// invariant at the event layer: after rollback there's no orphan store
/// event claiming a root that never received a matching assign.
///
/// Note: `store_kino` also writes a content blob under `.kinora/store/`.
/// A blob introduced by this call is intentionally NOT rolled back — the
/// store is content-addressed and dedup-safe, so a leaked blob is benign
/// and will be reaped by the GC pass (hxmw-6). The on-disk event set
/// stays consistent, which is what "atomic pair" means for the ledger.
fn pair_assign_with_rollback(
    kin_root: &Path,
    stored: &StoredKino,
    assign: &AssignEvent,
) -> Result<(), CliError> {
    match write_assign(kin_root, assign) {
        Ok(_) => Ok(()),
        Err(assign_err) => {
            if stored.was_new_lineage
                && let Ok(h) = stored.event.event_hash()
            {
                let _ = fs::remove_file(staged_event_path(kin_root, &h));
            }
            Err(assign_err.into())
        }
    }
}

/// Format the one-line human summary printed after a successful `kinora
/// store`. Under the staged-ledger layout each event lives in its own file
/// keyed by the event hash, so `event=<shorthash>` is the precise wording;
/// the prior "lineage" terminology is a carryover from the per-lineage
/// ledger layout and has been retired from the UI. The `StoredKino.lineage`
/// field is kept under that name for one release so programmatic callers
/// aren't broken — see kinora-6395.
pub fn format_store_summary(stored: &StoredKino) -> String {
    let suffix = if stored.was_new_lineage { " (new event)" } else { "" };
    format!(
        "stored kind={} id={} hash={} event={}{}",
        stored.event.kind,
        stored.event.id,
        stored.event.hash,
        stored.lineage,
        suffix,
    )
}

fn read_content(path: Option<&str>) -> Result<Vec<u8>, CliError> {
    match path {
        Some(p) => Ok(fs::read(p)?),
        None => {
            let mut buf = Vec::new();
            io::stdin().read_to_end(&mut buf)?;
            Ok(buf)
        }
    }
}

/// Parse kinograph bytes, resolve name references to ids against the
/// current ledger, and re-serialize. The on-disk blob is then
/// authoritative by id even if the author wrote names.
fn normalize_kinograph_content(kin_root: &Path, raw: &[u8]) -> Result<Vec<u8>, CliError> {
    let kinograph = Kinograph::parse(raw)?;
    let resolver = Resolver::load(kin_root)?;
    let resolved = kinograph.resolve_names(&resolver)?;
    Ok(resolved.to_styx()?.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kinora::init::init;
    use kinora::ledger::Ledger;
    use std::fs;
    use std::str::FromStr;
    use tempfile::TempDir;

    fn repo() -> TempDir {
        let tmp = TempDir::new().unwrap();
        init(tmp.path(), "https://example.com/x.git").unwrap();
        // Tests pass `author: Some("YJ")` in base_args so they don't depend
        // on the host's git config.
        tmp
    }

    fn base_args(kind: &str, path: &str) -> StoreRunArgs {
        StoreRunArgs {
            kind: kind.into(),
            path: Some(path.into()),
            provenance: "unit-test".into(),
            name: Some("doc".into()),
            id: None,
            parents: None,
            draft: false,
            author: Some("YJ".into()),
            metadata: vec![],
            root: None,
        }
    }

    #[test]
    fn store_from_file_writes_blob_and_event() {
        let tmp = repo();
        let src = tmp.path().join("note.md");
        fs::write(&src, b"hello kino").unwrap();

        let args = base_args("markdown", src.to_str().unwrap());
        let stored = run_store(tmp.path(), args).unwrap();
        assert!(stored.was_new_lineage);
        assert_eq!(stored.event.kind, "markdown");
        assert_eq!(stored.event.author, "YJ");
        assert_eq!(stored.event.metadata.get("name").unwrap(), "doc");
        let events = Ledger::new(kinora_root(tmp.path()))
            .read_all_events()
            .unwrap();
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn draft_flag_sets_metadata_draft_true() {
        let tmp = repo();
        let src = tmp.path().join("draft.md");
        fs::write(&src, b"wip").unwrap();

        let mut args = base_args("markdown", src.to_str().unwrap());
        args.draft = true;
        let stored = run_store(tmp.path(), args).unwrap();
        assert_eq!(stored.event.metadata.get("draft").unwrap(), "true");
    }

    #[test]
    fn metadata_flags_parse_into_event() {
        let tmp = repo();
        let src = tmp.path().join("tagged.md");
        fs::write(&src, b"x").unwrap();

        let mut args = base_args("markdown", src.to_str().unwrap());
        args.metadata = vec!["title=Hello".into(), "tags=one,two".into()];
        let stored = run_store(tmp.path(), args).unwrap();
        assert_eq!(stored.event.metadata.get("title").unwrap(), "Hello");
        assert_eq!(stored.event.metadata.get("tags").unwrap(), "one,two");
    }

    #[test]
    fn invalid_metadata_flag_rejected() {
        let tmp = repo();
        let src = tmp.path().join("x.md");
        fs::write(&src, b"x").unwrap();

        let mut args = base_args("markdown", src.to_str().unwrap());
        args.metadata = vec!["no-equals".into()];
        let err = run_store(tmp.path(), args).unwrap_err();
        assert!(matches!(err, CliError::InvalidMetadataFlag { .. }));
    }

    #[test]
    fn errors_when_run_outside_kinora_repo() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("x.md");
        fs::write(&src, b"x").unwrap();
        let args = base_args("markdown", src.to_str().unwrap());
        let err = run_store(tmp.path(), args).unwrap_err();
        assert!(matches!(err, CliError::NotInKinoraRepo { .. }));
    }

    #[test]
    fn author_unresolved_when_flag_missing_and_no_git_name() {
        let tmp = repo();
        let src = tmp.path().join("x.md");
        fs::write(&src, b"x").unwrap();

        // tmp has no git repo initialized → resolve_author_from_git returns None.
        let mut args = base_args("markdown", src.to_str().unwrap());
        args.author = None;
        let err = run_store(tmp.path(), args).unwrap_err();
        assert!(matches!(err, CliError::AuthorUnresolved));
    }

    #[test]
    fn draft_flag_conflicts_with_metadata_draft_value() {
        let tmp = repo();
        let src = tmp.path().join("x.md");
        fs::write(&src, b"x").unwrap();

        let mut args = base_args("markdown", src.to_str().unwrap());
        args.draft = true;
        args.metadata = vec!["draft=false".into()];
        let err = run_store(tmp.path(), args).unwrap_err();
        assert!(matches!(err, CliError::ConflictingDraftFlag));
    }

    #[test]
    fn metadata_flag_trims_whitespace_around_key() {
        let tmp = repo();
        let src = tmp.path().join("x.md");
        fs::write(&src, b"x").unwrap();

        let mut args = base_args("markdown", src.to_str().unwrap());
        args.metadata = vec!["  title  =Hello".into()];
        let stored = run_store(tmp.path(), args).unwrap();
        assert_eq!(stored.event.metadata.get("title").unwrap(), "Hello");
    }

    #[test]
    fn kinograph_kind_rewrites_names_to_ids_before_store() {
        let tmp = repo();
        // Seed a kino the kinograph can reference by name.
        let first_content = tmp.path().join("target.md");
        fs::write(&first_content, b"target body").unwrap();
        let mut first_args = base_args("markdown", first_content.to_str().unwrap());
        first_args.name = Some("target".into());
        let first = run_store(tmp.path(), first_args).unwrap();

        // Kinograph content references by name only. Store should
        // rewrite the id slot to the stored kino's identity hash.
        let kg_path = tmp.path().join("doc.kinograph");
        fs::write(&kg_path, b"entries ({id target})").unwrap();
        let mut kg_args = base_args("kinograph", kg_path.to_str().unwrap());
        kg_args.name = Some("doc".into());
        let stored = run_store(tmp.path(), kg_args).unwrap();

        let blob_path = kinora::paths::find_blob_path(
            &kinora_root(tmp.path()),
            &kinora::hash::Hash::from_str(&stored.event.hash).unwrap(),
        )
        .unwrap();
        let written = fs::read_to_string(blob_path).unwrap();
        assert!(
            written.contains(&first.event.id),
            "stored kinograph should contain the resolved id, got: {written}"
        );
        assert!(written.contains("name target"), "should preserve name hint: {written}");
    }

    #[test]
    fn kinograph_kind_errors_on_ambiguous_name() {
        let tmp = repo();
        for (body, name) in [(b"a" as &[u8], "dup"), (b"b", "dup")] {
            let src = tmp.path().join(format!("{name}-{}.md", body[0] as char));
            fs::write(&src, body).unwrap();
            let mut a = base_args("markdown", src.to_str().unwrap());
            a.name = Some(name.into());
            run_store(tmp.path(), a).unwrap();
        }
        let kg_path = tmp.path().join("doc.kinograph");
        fs::write(&kg_path, b"entries ({id dup})").unwrap();
        let mut args = base_args("kinograph", kg_path.to_str().unwrap());
        args.name = Some("doc".into());
        let err = run_store(tmp.path(), args).unwrap_err();
        assert!(matches!(err, CliError::Kinograph(_)), "got: {err:?}");
    }

    #[test]
    fn kinograph_kind_errors_on_missing_name() {
        let tmp = repo();
        let kg_path = tmp.path().join("broken.kinograph");
        fs::write(&kg_path, b"entries ({id nobody})").unwrap();
        let mut args = base_args("kinograph", kg_path.to_str().unwrap());
        args.name = Some("doc".into());
        let err = run_store(tmp.path(), args).unwrap_err();
        assert!(matches!(err, CliError::Kinograph(_)), "got: {err:?}");
    }

    #[test]
    fn kinograph_kind_passes_through_hash_ids_unchanged() {
        let tmp = repo();
        let first_content = tmp.path().join("target.md");
        fs::write(&first_content, b"x").unwrap();
        let mut first_args = base_args("markdown", first_content.to_str().unwrap());
        first_args.name = Some("tgt".into());
        let first = run_store(tmp.path(), first_args).unwrap();

        let kg_path = tmp.path().join("doc.kinograph");
        fs::write(
            &kg_path,
            format!("entries ({{id {}}})", first.event.id).as_bytes(),
        )
        .unwrap();
        let mut kg_args = base_args("kinograph", kg_path.to_str().unwrap());
        kg_args.name = Some("doc".into());
        let stored = run_store(tmp.path(), kg_args).unwrap();

        let blob_path = kinora::paths::find_blob_path(
            &kinora_root(tmp.path()),
            &kinora::hash::Hash::from_str(&stored.event.hash).unwrap(),
        )
        .unwrap();
        let written = fs::read_to_string(blob_path).unwrap();
        assert!(written.contains(&first.event.id));
    }

    fn stubbed_stored_kino(was_new_lineage: bool) -> kinora::kino::StoredKino {
        use kinora::event::Event;
        use std::collections::BTreeMap as Btm;
        kinora::kino::StoredKino {
            event: Event::new_store(
                "markdown".into(),
                "aa".repeat(32),
                "bb".repeat(32),
                vec![],
                "2026-04-19T10:00:00Z".into(),
                "yj".into(),
                "unit".into(),
                Btm::new(),
            ),
            lineage: "deadbeef".into(),
            was_new_lineage,
        }
    }

    #[test]
    fn format_store_summary_uses_event_wording_for_new_events() {
        let stored = stubbed_stored_kino(true);
        let summary = format_store_summary(&stored);
        assert!(summary.contains(" (new event)"), "expected `(new event)`: {summary}");
        assert!(summary.contains("event=deadbeef"));
        assert!(
            !summary.contains("lineage"),
            "lineage wording should be retired: {summary}",
        );
    }

    #[test]
    fn format_store_summary_omits_suffix_on_idempotent_restore() {
        let stored = stubbed_stored_kino(false);
        let summary = format_store_summary(&stored);
        assert!(
            !summary.contains("(new"),
            "no new-event suffix on idempotent re-store: {summary}",
        );
        assert!(summary.contains("event=deadbeef"));
    }

    #[test]
    fn format_store_summary_has_expected_shape() {
        let stored = stubbed_stored_kino(true);
        let summary = format_store_summary(&stored);
        let expected = format!(
            "stored kind=markdown id={} hash={} event=deadbeef (new event)",
            "aa".repeat(32),
            "bb".repeat(32),
        );
        assert_eq!(summary, expected);
    }

    // ---- --root atomic-pair tests (g08g Phase B) ----

    #[test]
    fn store_without_root_writes_exactly_one_event() {
        let tmp = repo();
        let src = tmp.path().join("x.md");
        fs::write(&src, b"x").unwrap();

        run_store(tmp.path(), base_args("markdown", src.to_str().unwrap())).unwrap();

        let events = Ledger::new(kinora_root(tmp.path()))
            .read_all_events()
            .unwrap();
        assert_eq!(events.len(), 1);
        assert!(events[0].is_store_event());
    }

    #[test]
    fn store_with_root_writes_both_events_as_pair() {
        use kinora::assign::{EVENT_KIND_ASSIGN, META_TARGET_ROOT};

        let tmp = repo();
        let src = tmp.path().join("x.md");
        fs::write(&src, b"paired").unwrap();

        let mut args = base_args("markdown", src.to_str().unwrap());
        args.root = Some("main".into());
        let stored = run_store(tmp.path(), args).unwrap();

        let events = Ledger::new(kinora_root(tmp.path()))
            .read_all_events()
            .unwrap();
        assert_eq!(events.len(), 2);

        let assign = events
            .iter()
            .find(|e| e.event_kind == EVENT_KIND_ASSIGN)
            .expect("assign event must be present");
        assert_eq!(assign.id, stored.event.id);
        assert_eq!(assign.metadata.get(META_TARGET_ROOT).unwrap(), "main");
        assert!(assign.parents.is_empty(), "birth-assign has no supersedes");
    }

    #[test]
    fn store_with_empty_root_rejected_before_write() {
        let tmp = repo();
        let src = tmp.path().join("x.md");
        fs::write(&src, b"x").unwrap();

        let mut args = base_args("markdown", src.to_str().unwrap());
        args.root = Some(String::new());
        let err = run_store(tmp.path(), args).unwrap_err();
        assert!(matches!(err, CliError::EmptyRoot), "got {err:?}");

        // Nothing written to disk.
        let events = Ledger::new(kinora_root(tmp.path()))
            .read_all_events()
            .unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn store_with_root_rolls_back_store_event_on_assign_failure() {
        // Injects a failure at the second write of the atomic pair by
        // pre-placing a directory at the expected assign event staged-file
        // path, which makes `fs::rename(tmp, target)` fail — exactly the
        // failure mode we must roll back from.
        use kinora::assign::AssignEvent;
        use kinora::paths::staged_event_path;

        let tmp = repo();
        let kin_root = kinora_root(tmp.path());
        let src = tmp.path().join("x.md");
        fs::write(&src, b"rollback-me").unwrap();

        // Pre-compute the assign event hash with the same ts/author/root
        // that run_store will produce. We can't predict jiff::Timestamp::now,
        // so we simulate the pair directly via store_kino + pair_assign_with_rollback
        // with a fixed ts.
        let content = b"rollback-me-inner".to_vec();
        let params = StoreKinoParams {
            kind: "markdown".into(),
            content: content.clone(),
            author: "YJ".into(),
            provenance: "unit-test".into(),
            ts: "2026-04-19T10:05:00Z".into(),
            metadata: std::collections::BTreeMap::from([("name".into(), "rb".into())]),
            id: None,
            parents: vec![],
        };
        let stored = kinora::kino::store_kino(&kin_root, params).unwrap();
        assert!(stored.was_new_lineage);

        // Build the assign we're going to write, then sabotage its target
        // path with a directory to force `fs::rename` to fail.
        let assign = AssignEvent {
            kino_id: stored.event.id.clone(),
            target_root: "main".into(),
            supersedes: vec![],
            author: "YJ".into(),
            ts: "2026-04-19T10:05:00Z".into(),
            provenance: "unit-test".into(),
        };
        let assign_hash = assign.event_hash().unwrap();
        let assign_path = staged_event_path(&kin_root, &assign_hash);
        fs::create_dir_all(&assign_path).unwrap();
        // Non-empty dir blocks fs::rename more reliably across platforms.
        fs::write(assign_path.join("blocker"), b"x").unwrap();

        let err = pair_assign_with_rollback(&kin_root, &stored, &assign).unwrap_err();
        assert!(matches!(err, CliError::Assign(_)), "got {err:?}");

        // Rollback: store event staged file must be gone.
        let store_event_hash = stored.event.event_hash().unwrap();
        let store_event_path = staged_event_path(&kin_root, &store_event_hash);
        assert!(
            !store_event_path.exists(),
            "store event staged file should have been rolled back: {}",
            store_event_path.display()
        );
    }

    #[test]
    fn version_event_with_existing_parent_succeeds() {
        let tmp = repo();
        let src1 = tmp.path().join("v1.md");
        fs::write(&src1, b"v1").unwrap();
        let first = run_store(tmp.path(), base_args("markdown", src1.to_str().unwrap())).unwrap();

        let src2 = tmp.path().join("v2.md");
        fs::write(&src2, b"v2").unwrap();
        let mut args = base_args("markdown", src2.to_str().unwrap());
        args.id = Some(first.event.id.clone());
        args.parents = Some(first.event.hash.clone());
        let second = run_store(tmp.path(), args).unwrap();
        assert_eq!(second.event.id, first.event.id);
        assert_eq!(second.event.parents, vec![first.event.hash]);
    }
}
