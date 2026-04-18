use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use kinora::kino::StoreKinoError;
use kinora::paths::KINORA_DIR;
use kinora::resolve::ResolveError;

#[derive(Debug)]
pub enum CliError {
    Io(io::Error),
    NotInKinoraRepo { start: PathBuf },
    InvalidMetadataFlag { got: String },
    ConflictingDraftFlag,
    AuthorUnresolved,
    StoreKino(StoreKinoError),
    Resolve(ResolveError),
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CliError::Io(e) => write!(f, "io error: {e}"),
            CliError::NotInKinoraRepo { start } => write!(
                f,
                "not in a kinora repo: no `{KINORA_DIR}/` found above {}",
                start.display()
            ),
            CliError::InvalidMetadataFlag { got } => write!(
                f,
                "--metadata expects KEY=VALUE, got `{got}`"
            ),
            CliError::ConflictingDraftFlag => write!(
                f,
                "--draft conflicts with `-m draft=…`; pass only one"
            ),
            CliError::AuthorUnresolved => write!(
                f,
                "could not resolve author: pass --author NAME or set git `user.name`"
            ),
            CliError::StoreKino(e) => write!(f, "{e}"),
            CliError::Resolve(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for CliError {}

impl From<io::Error> for CliError {
    fn from(e: io::Error) -> Self {
        CliError::Io(e)
    }
}

impl From<StoreKinoError> for CliError {
    fn from(e: StoreKinoError) -> Self {
        CliError::StoreKino(e)
    }
}

impl From<ResolveError> for CliError {
    fn from(e: ResolveError) -> Self {
        CliError::Resolve(e)
    }
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
