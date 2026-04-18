use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read};
use std::path::Path;

use kinora::author::resolve_author_from_git;
use kinora::kino::{store_kino, StoreKinoParams, StoredKino};
use kinora::paths::kinora_root;

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

    let content = read_content(args.path.as_deref())?;

    let mut metadata: BTreeMap<String, String> = BTreeMap::new();
    if let Some(name) = args.name {
        metadata.insert("name".into(), name);
    }
    if args.draft {
        metadata.insert("draft".into(), "true".into());
    }
    for kv in &args.metadata {
        let (k, v) = parse_metadata_flag(kv)?;
        metadata.insert(k, v);
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

#[cfg(test)]
mod tests {
    use super::*;
    use kinora::init::init;
    use kinora::ledger::Ledger;
    use std::fs;
    use tempfile::TempDir;

    fn repo() -> TempDir {
        let tmp = TempDir::new().unwrap();
        init(tmp.path(), "https://example.com/x.git").unwrap();
        // Pre-set an author so tests don't depend on host git config.
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
