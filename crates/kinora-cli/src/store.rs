use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read};
use std::path::Path;

use kinora::author::resolve_author_from_git;
use kinora::kino::{store_kino, StoreKinoParams, StoredKino};
use kinora::kinograph::Kinograph;
use kinora::paths::kinora_root;
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
}

pub fn run_store(cwd: &Path, args: StoreRunArgs) -> Result<StoredKino, CliError> {
    let repo_root = find_repo_root(cwd)?;
    let kin_root = kinora_root(&repo_root);

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
        author,
        provenance: args.provenance,
        ts,
        metadata,
        id: args.id,
        parents,
    };
    let stored = store_kino(&kin_root, params)?;
    Ok(stored)
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
            .read_lineage(&stored.lineage)
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

        let blob_path = kinora::paths::store_blob_path(
            &kinora_root(tmp.path()),
            &kinora::hash::Hash::from_str(&stored.event.hash).unwrap(),
        );
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

        let blob_path = kinora::paths::store_blob_path(
            &kinora_root(tmp.path()),
            &kinora::hash::Hash::from_str(&stored.event.hash).unwrap(),
        );
        let written = fs::read_to_string(blob_path).unwrap();
        assert!(written.contains(&first.event.id));
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
