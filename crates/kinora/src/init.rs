use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use std::collections::BTreeMap;

use crate::config::{Config, ConfigError, RootPolicy, DEFAULT_INBOX_POLICY};
use crate::paths::{config_path, kinora_root, ledger_dir, store_dir};

#[derive(Debug)]
pub enum InitError {
    Io(io::Error),
    AlreadyInitialized { path: PathBuf },
    RepoUrlUnresolved,
    Git(String),
    Config(ConfigError),
}

impl std::fmt::Display for InitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InitError::Io(e) => write!(f, "init io error: {e}"),
            InitError::AlreadyInitialized { path } => {
                write!(f, ".kinora/ already exists at {}", path.display())
            }
            InitError::RepoUrlUnresolved => write!(
                f,
                "could not determine repo-url: no --repo-url and no `origin` remote found"
            ),
            InitError::Git(m) => write!(f, "git error: {m}"),
            InitError::Config(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for InitError {}

impl From<io::Error> for InitError {
    fn from(e: io::Error) -> Self {
        InitError::Io(e)
    }
}

impl From<ConfigError> for InitError {
    fn from(e: ConfigError) -> Self {
        InitError::Config(e)
    }
}

/// Initialize `.kinora/` under `repo_root` with the given `repo_url`.
///
/// Creates `config.styx` plus empty `store/` and `ledger/` directories.
/// HEAD is not written here — the first `store` call mints a lineage file
/// and sets HEAD.
///
/// Refuses if `.kinora/` already exists.
pub fn init(repo_root: &Path, repo_url: &str) -> Result<Config, InitError> {
    let root = kinora_root(repo_root);
    if root.exists() {
        return Err(InitError::AlreadyInitialized { path: root });
    }
    fs::create_dir_all(&root)?;
    fs::create_dir_all(store_dir(&root))?;
    fs::create_dir_all(ledger_dir(&root))?;
    let cfg = Config {
        repo_url: repo_url.to_owned(),
        roots: BTreeMap::from([(
            "inbox".to_owned(),
            RootPolicy::MaxAge(DEFAULT_INBOX_POLICY.to_owned()),
        )]),
    };
    fs::write(config_path(&root), cfg.to_styx()?)?;
    Ok(cfg)
}

/// Read `origin` remote URL from a git repository at `repo_root`, if any.
pub fn resolve_repo_url_from_git(repo_root: &Path) -> Result<Option<String>, InitError> {
    let repo = match gix::open(repo_root) {
        Ok(r) => r,
        Err(_) => return Ok(None),
    };
    let remote = match repo.find_remote("origin") {
        Ok(r) => r,
        Err(_) => return Ok(None),
    };
    match remote.url(gix::remote::Direction::Fetch) {
        Some(url) => Ok(Some(url.to_bstring().to_string())),
        None => Ok(None),
    }
}

/// Combined init: prefer `repo_url_flag` if present; otherwise read the
/// `origin` remote URL from git at `repo_root`. Errors if neither resolves.
pub fn init_with_git_fallback(
    repo_root: &Path,
    repo_url_flag: Option<&str>,
) -> Result<Config, InitError> {
    let url = match repo_url_flag {
        Some(u) => u.to_owned(),
        None => resolve_repo_url_from_git(repo_root)?.ok_or(InitError::RepoUrlUnresolved)?,
    };
    init(repo_root, &url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::{head_path, kinora_root};
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn init_creates_kinora_layout() {
        let tmp = TempDir::new().unwrap();
        let cfg = init(tmp.path(), "https://example.com/x.git").unwrap();
        assert_eq!(cfg.repo_url, "https://example.com/x.git");
        let root = kinora_root(tmp.path());
        assert!(root.is_dir());
        assert!(config_path(&root).is_file());
        assert!(store_dir(&root).is_dir());
        assert!(ledger_dir(&root).is_dir());
        // HEAD not yet written (minted on first store)
        assert!(!head_path(&root).exists());
    }

    #[test]
    fn init_stores_config_parseably() {
        let tmp = TempDir::new().unwrap();
        init(tmp.path(), "https://example.com/x.git").unwrap();
        let text = fs::read_to_string(config_path(&kinora_root(tmp.path()))).unwrap();
        let parsed = Config::from_styx(&text).unwrap();
        assert_eq!(parsed.repo_url, "https://example.com/x.git");
    }

    #[test]
    fn init_writes_explicit_inbox_root_block() {
        let tmp = TempDir::new().unwrap();
        init(tmp.path(), "https://example.com/x.git").unwrap();
        let text = fs::read_to_string(config_path(&kinora_root(tmp.path()))).unwrap();
        assert!(
            text.contains("roots"),
            "expected `roots` block in initial config.styx, got:\n{text}"
        );
        assert!(
            text.contains("inbox"),
            "expected inbox declaration in initial config.styx, got:\n{text}"
        );
        let parsed = Config::from_styx(&text).unwrap();
        assert_eq!(
            parsed.roots.get("inbox"),
            Some(&RootPolicy::MaxAge(DEFAULT_INBOX_POLICY.to_owned()))
        );
    }

    #[test]
    fn init_refuses_if_kinora_dir_already_present() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(kinora_root(tmp.path())).unwrap();
        let err = init(tmp.path(), "https://example.com/x.git").unwrap_err();
        assert!(matches!(err, InitError::AlreadyInitialized { .. }));
    }

    #[test]
    fn resolve_returns_none_for_non_git_dir() {
        let tmp = TempDir::new().unwrap();
        let got = resolve_repo_url_from_git(tmp.path()).unwrap();
        assert!(got.is_none());
    }

    fn init_git_repo_with_origin(path: &Path, origin_url: &str) {
        gix::init(path).expect("gix init");
        let cfg = path.join(".git").join("config");
        let existing = fs::read_to_string(&cfg).unwrap_or_default();
        let appended = format!(
            "{existing}[remote \"origin\"]\n\turl = {origin_url}\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n"
        );
        fs::write(&cfg, appended).unwrap();
    }

    #[test]
    fn resolve_returns_origin_url_from_git_config() {
        let tmp = TempDir::new().unwrap();
        init_git_repo_with_origin(tmp.path(), "https://github.com/edger-dev/kinora");
        let got = resolve_repo_url_from_git(tmp.path()).unwrap();
        assert_eq!(got.as_deref(), Some("https://github.com/edger-dev/kinora"));
    }

    #[test]
    fn resolve_returns_none_when_no_origin_remote() {
        let tmp = TempDir::new().unwrap();
        gix::init(tmp.path()).unwrap();
        let got = resolve_repo_url_from_git(tmp.path()).unwrap();
        assert!(got.is_none());
    }

    #[test]
    fn init_with_git_fallback_prefers_flag() {
        let tmp = TempDir::new().unwrap();
        init_git_repo_with_origin(tmp.path(), "https://remote.example.com/x");
        let cfg = init_with_git_fallback(
            tmp.path(),
            Some("https://explicit.example.com/y"),
        ).unwrap();
        assert_eq!(cfg.repo_url, "https://explicit.example.com/y");
    }

    #[test]
    fn init_with_git_fallback_reads_origin_when_no_flag() {
        let tmp = TempDir::new().unwrap();
        init_git_repo_with_origin(tmp.path(), "https://remote.example.com/x");
        let cfg = init_with_git_fallback(tmp.path(), None).unwrap();
        assert_eq!(cfg.repo_url, "https://remote.example.com/x");
    }

    #[test]
    fn init_with_git_fallback_errors_when_nothing_resolves() {
        let tmp = TempDir::new().unwrap();
        let err = init_with_git_fallback(tmp.path(), None).unwrap_err();
        assert!(matches!(err, InitError::RepoUrlUnresolved));
    }
}
