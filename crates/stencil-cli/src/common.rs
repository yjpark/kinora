use std::path::{Path, PathBuf};

use kinora::paths::KINORA_DIR;

/// CLI-layer error. Wraps [`stencil::StencilError`] and carries CLI-only
/// variants. The binary turns these into `rootcause` reports for display.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error(transparent)]
    Stencil(#[from] stencil::StencilError),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Resolve(#[from] kinora::resolve::ResolveError),

    #[error("not in a kinora repo: no `{}/` found above {}", KINORA_DIR, .start.display())]
    NotInKinoraRepo { start: PathBuf },

    /// A `sync` path argument that does not exist on disk.
    #[error("path does not exist: {path}", path = .path.display())]
    PathNotFound { path: PathBuf },

    /// A subcommand whose engine has not landed yet. Scaffolding ships the CLI
    /// surface ahead of the implementing beans (kinora-exay / kinora-guv8).
    #[error("`stencil {command}` is not implemented yet (tracked in beans)")]
    NotImplemented { command: &'static str },
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

/// Resolve the repo root for a CLI invocation. With `override_path` set, treat
/// it verbatim as the repo root and require `.kinora/` directly under it (no
/// walk-up); otherwise walk up from `cwd`. Mirrors kinora-cli's `-C` handling.
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

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
    fn resolve_repo_root_override_requires_kinora_directly_under() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join(KINORA_DIR)).unwrap();
        assert_eq!(
            resolve_repo_root(Path::new("/unused"), Some(tmp.path())).unwrap(),
            tmp.path()
        );
        let nested = tmp.path().join("a");
        fs::create_dir_all(&nested).unwrap();
        let err = resolve_repo_root(tmp.path(), Some(&nested)).unwrap_err();
        assert!(matches!(err, CliError::NotInKinoraRepo { .. }));
    }
}
