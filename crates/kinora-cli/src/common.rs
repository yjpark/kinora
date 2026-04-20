use std::io;
use std::path::{Path, PathBuf};

use kinora::assign::AssignError;
use kinora::clone::CloneError;
use kinora::commit::CommitError;
use kinora::config::ConfigError;
use kinora::git_state::{EnumError, ExtractError};
use kinora::kino::StoreKinoError;
use kinora::kinograph::KinographError;
use kinora::ledger::LedgerError;
use kinora::paths::KINORA_DIR;
use kinora::reformat::ReformatError;
use kinora::repack::RepackError;
use kinora::render::RenderError;
use kinora::resolve::ResolveError;
use kinora::root::RootError;
use kinora::store::StoreError;

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("not in a kinora repo: no `{}/` found above {}", KINORA_DIR, .start.display())]
    NotInKinoraRepo { start: PathBuf },
    #[error("--metadata expects KEY=VALUE, got `{got}`")]
    InvalidMetadataFlag { got: String },
    #[error("--draft conflicts with `-m draft=…`; pass only one")]
    ConflictingDraftFlag,
    #[error("could not resolve author: pass --author NAME or set git `user.name`")]
    AuthorUnresolved,
    #[error("could not resolve cache root: set $XDG_CACHE_HOME or $HOME, or pass --cache-dir")]
    CacheHomeUnresolved,
    #[error("--root must be a non-empty root name")]
    EmptyRoot,
    #[error("`kinora clone` takes src/dst directly — `-C`/`--repo-root` is not supported here")]
    CloneWithRepoRoot,
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    StoreKino(#[from] StoreKinoError),
    #[error(transparent)]
    Resolve(#[from] ResolveError),
    #[error(transparent)]
    Kinograph(#[from] KinographError),
    #[error(transparent)]
    Render(#[from] RenderError),
    #[error(transparent)]
    Ledger(#[from] LedgerError),
    #[error(transparent)]
    Commit(#[from] CommitError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Root(#[from] RootError),
    #[error(transparent)]
    Assign(#[from] AssignError),
    #[error(transparent)]
    Reformat(#[from] ReformatError),
    #[error(transparent)]
    Clone(#[from] CloneError),
    #[error(transparent)]
    Repack(#[from] RepackError),
    #[error(transparent)]
    Extract(#[from] ExtractError),
    #[error(transparent)]
    GitEnum(#[from] EnumError),
}

/// Walk up from `start` looking for a directory that contains `.kinora/`.
/// Returns that directory (the repo root), not `.kinora/` itself.
pub fn find_repo_root(start: &Path) -> Result<PathBuf, CliError> {
    let mut cur = start;
    loop {
        if cur.join(KINORA_DIR).is_dir() {
            return Ok(cur.to_path_buf());
        }
        match cur.parent() {
            Some(p) => cur = p,
            None => return Err(CliError::NotInKinoraRepo { start: start.to_path_buf() }),
        }
    }
}

/// Resolve the repo root for a CLI invocation.
///
/// When `override_path` is `Some`, treat it verbatim as the repo root and
/// require `.kinora/` to sit directly under it — no walk-up. When `None`,
/// fall back to walking up from `cwd` via [`find_repo_root`].
///
/// This is the entry point main.rs uses for the global `--repo-root` /
/// `-C` flag; tests exercise both branches directly.
pub fn resolve_repo_root(cwd: &Path, override_path: Option<&Path>) -> Result<PathBuf, CliError> {
    match override_path {
        Some(p) => {
            if p.join(KINORA_DIR).is_dir() {
                Ok(p.to_path_buf())
            } else {
                Err(CliError::NotInKinoraRepo { start: p.to_path_buf() })
            }
        }
        None => find_repo_root(cwd),
    }
}

/// Parse a single `KEY=VALUE` string. The key is trimmed; empty keys
/// are rejected. The value may be empty (explicit empty string) and may
/// contain `=` (split on the first `=` only).
pub fn parse_metadata_flag(s: &str) -> Result<(String, String), CliError> {
    match s.split_once('=') {
        Some((k, v)) => {
            let k = k.trim();
            if k.is_empty() {
                Err(CliError::InvalidMetadataFlag { got: s.to_owned() })
            } else {
                Ok((k.to_owned(), v.to_owned()))
            }
        }
        None => Err(CliError::InvalidMetadataFlag { got: s.to_owned() }),
    }
}

/// Parse comma-separated parent hashes; empty input yields an empty vec.
/// Whitespace around each hash is trimmed; empty entries (from stray
/// commas) are dropped.
pub fn parse_parents(s: Option<&str>) -> Vec<String> {
    s.unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn find_repo_root_matches_cwd_when_kinora_present() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join(KINORA_DIR)).unwrap();
        assert_eq!(find_repo_root(tmp.path()).unwrap(), tmp.path());
    }

    #[test]
    fn find_repo_root_walks_up_to_parent() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join(KINORA_DIR)).unwrap();
        let nested = tmp.path().join("a").join("b");
        fs::create_dir_all(&nested).unwrap();
        assert_eq!(find_repo_root(&nested).unwrap(), tmp.path());
    }

    #[test]
    fn find_repo_root_errors_when_no_kinora_anywhere() {
        let tmp = TempDir::new().unwrap();
        let err = find_repo_root(tmp.path()).unwrap_err();
        assert!(matches!(err, CliError::NotInKinoraRepo { .. }));
    }

    #[test]
    fn resolve_repo_root_override_returns_verbatim_when_kinora_present() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join(KINORA_DIR)).unwrap();
        let resolved = resolve_repo_root(Path::new("/unused"), Some(tmp.path())).unwrap();
        assert_eq!(resolved, tmp.path());
    }

    #[test]
    fn resolve_repo_root_override_does_not_walk_up() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join(KINORA_DIR)).unwrap();
        let nested = tmp.path().join("a").join("b");
        fs::create_dir_all(&nested).unwrap();
        let err = resolve_repo_root(tmp.path(), Some(&nested)).unwrap_err();
        match err {
            CliError::NotInKinoraRepo { start } => assert_eq!(start, nested),
            other => panic!("expected NotInKinoraRepo, got {other:?}"),
        }
    }

    #[test]
    fn resolve_repo_root_override_errors_for_nonexistent_path() {
        let err =
            resolve_repo_root(Path::new("/unused"), Some(Path::new("/nonexistent"))).unwrap_err();
        assert!(matches!(err, CliError::NotInKinoraRepo { .. }));
    }

    #[test]
    fn resolve_repo_root_none_walks_up_from_cwd() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join(KINORA_DIR)).unwrap();
        let nested = tmp.path().join("a").join("b");
        fs::create_dir_all(&nested).unwrap();
        let resolved = resolve_repo_root(&nested, None).unwrap();
        assert_eq!(resolved, tmp.path());
    }

    #[test]
    fn parse_metadata_flag_splits_on_first_equals() {
        assert_eq!(
            parse_metadata_flag("key=value=extra").unwrap(),
            ("key".into(), "value=extra".into())
        );
    }

    #[test]
    fn parse_metadata_flag_accepts_empty_value() {
        assert_eq!(
            parse_metadata_flag("k=").unwrap(),
            ("k".into(), "".into())
        );
    }

    #[test]
    fn parse_metadata_flag_rejects_empty_key() {
        assert!(parse_metadata_flag("=v").is_err());
    }

    #[test]
    fn parse_metadata_flag_rejects_no_equals() {
        assert!(parse_metadata_flag("plain").is_err());
    }

    #[test]
    fn parse_parents_splits_comma_list() {
        assert_eq!(
            parse_parents(Some("a,b,c")),
            vec!["a".to_string(), "b".into(), "c".into()]
        );
    }

    #[test]
    fn parse_parents_trims_and_drops_empties() {
        assert_eq!(
            parse_parents(Some(" a , ,b ,")),
            vec!["a".to_string(), "b".into()]
        );
    }

    #[test]
    fn parse_parents_empty_for_none_or_blank() {
        assert!(parse_parents(None).is_empty());
        assert!(parse_parents(Some("")).is_empty());
    }
}
