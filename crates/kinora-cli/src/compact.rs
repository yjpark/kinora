use std::path::Path;

use kinora::author::resolve_author_from_git;
use kinora::compact::{compact, CompactParams, CompactResult};
use kinora::paths::kinora_root;

use crate::common::{find_repo_root, CliError};

pub const DEFAULT_ROOT_NAME: &str = "main";
pub const DEFAULT_PROVENANCE: &str = "compact";

pub struct CompactRunArgs {
    pub root: Option<String>,
    pub author: Option<String>,
    pub provenance: Option<String>,
}

pub fn run_compact(cwd: &Path, args: CompactRunArgs) -> Result<CompactResult, CliError> {
    let repo_root = find_repo_root(cwd)?;
    let kin_root = kinora_root(&repo_root);

    let author = match args.author {
        Some(a) => a,
        None => resolve_author_from_git(&repo_root).ok_or(CliError::AuthorUnresolved)?,
    };
    let provenance = args.provenance.unwrap_or_else(|| DEFAULT_PROVENANCE.to_owned());
    let ts = jiff::Timestamp::now().to_string();
    let root_name = args.root.unwrap_or_else(|| DEFAULT_ROOT_NAME.to_owned());

    let params = CompactParams { author, provenance, ts };
    let result = compact(&kin_root, &root_name, params)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kinora::hash::Hash;
    use kinora::init::init;
    use kinora::kino::{store_kino, StoreKinoParams};
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn repo() -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        init(tmp.path(), "https://example.com/x.git").unwrap();
        let root = kinora_root(tmp.path());
        (tmp, root)
    }

    fn args() -> CompactRunArgs {
        CompactRunArgs {
            root: None,
            author: Some("YJ".into()),
            provenance: Some("cli-test".into()),
        }
    }

    fn store_md(root: &std::path::Path, content: &[u8], name: &str) {
        store_kino(
            root,
            StoreKinoParams {
                kind: "markdown".into(),
                content: content.to_vec(),
                author: "yj".into(),
                provenance: "cli-test".into(),
                ts: "2026-04-19T10:00:00Z".into(),
                metadata: BTreeMap::from([("name".into(), name.into())]),
                id: None,
                parents: vec![],
            },
        )
        .unwrap();
    }

    #[test]
    fn run_compact_defaults_root_to_main() {
        let (tmp, kin) = repo();
        store_md(&kin, b"x", "x");

        let result = run_compact(tmp.path(), args()).unwrap();
        assert_eq!(result.root_name, "main");
        let hash: Hash = result.new_version.expect("new version");
        let pointer = std::fs::read_to_string(
            kinora::paths::root_pointer_path(&kin, "main"),
        )
        .unwrap();
        assert_eq!(pointer, hash.as_hex());
    }

    #[test]
    fn run_compact_uses_custom_root_name() {
        let (tmp, kin) = repo();
        store_md(&kin, b"x", "x");

        let mut a = args();
        a.root = Some("custom".into());
        let result = run_compact(tmp.path(), a).unwrap();
        assert_eq!(result.root_name, "custom");
        assert!(kinora::paths::root_pointer_path(&kin, "custom").is_file());
        assert!(!kinora::paths::root_pointer_path(&kin, "main").exists());
    }

    #[test]
    fn run_compact_errors_outside_kinora_repo() {
        let tmp = TempDir::new().unwrap();
        let err = run_compact(tmp.path(), args()).unwrap_err();
        assert!(matches!(err, CliError::NotInKinoraRepo { .. }));
    }

    #[test]
    fn run_compact_errors_when_author_unresolved() {
        let (tmp, _kin) = repo();
        let mut a = args();
        a.author = None;
        // No git user.name → AuthorUnresolved
        let err = run_compact(tmp.path(), a).unwrap_err();
        assert!(matches!(err, CliError::AuthorUnresolved));
    }
}
