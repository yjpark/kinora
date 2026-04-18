use std::path::{Path, PathBuf};

use kinora::cache_path::CachePath;
use kinora::config::Config;
use kinora::ledger::Ledger;
use kinora::paths::{config_path, kinora_root};
use kinora::render::{render_for_branch, write_book};
use kinora::resolve::Resolver;

use crate::common::{find_repo_root, CliError};

pub struct RenderRunArgs {
    pub cache_dir: Option<String>,
}

#[derive(Debug)]
pub struct RenderReport {
    pub cache_path: PathBuf,
    pub page_count: usize,
}

pub fn run_render(cwd: &Path, args: RenderRunArgs) -> Result<RenderReport, CliError> {
    let repo_root = find_repo_root(cwd)?;
    let kin_root = kinora_root(&repo_root);

    let config_text = std::fs::read_to_string(config_path(&kin_root))?;
    let config = Config::from_styx(&config_text)?;

    let cache = CachePath::from_repo_url(&config.repo_url);
    let cache_path = match args.cache_dir {
        Some(override_dir) => PathBuf::from(override_dir),
        None => default_cache_root()?.join(cache.subdir()),
    };

    let resolver = Resolver::load(&kin_root)?;
    let branch = current_branch_label(&kin_root)?;
    let book = render_for_branch(&resolver, &branch)?;
    let page_count = book.pages.len();

    let title = if cache.name.is_empty() {
        "kinora".to_owned()
    } else {
        cache.name.clone()
    };
    write_book(&cache_path, &title, &book)?;

    Ok(RenderReport { cache_path, page_count })
}

fn default_cache_root() -> Result<PathBuf, CliError> {
    if let Ok(x) = std::env::var("XDG_CACHE_HOME")
        && !x.is_empty()
    {
        return Ok(PathBuf::from(x).join("kinora"));
    }
    if let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
    {
        return Ok(PathBuf::from(home).join(".cache").join("kinora"));
    }
    Err(CliError::CacheHomeUnresolved)
}

/// Best-effort "branch" label. Without multi-branch support, falls back to
/// the current lineage shorthash — unique per originating HEAD, so the
/// single-branch layout doesn't collide with multi-branch output added later.
fn current_branch_label(kin_root: &Path) -> Result<String, CliError> {
    match Ledger::new(kin_root).current_lineage()? {
        Some(sh) => Ok(sh),
        None => Ok("main".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kinora::init::init;
    use kinora::kino::{store_kino, StoreKinoParams};
    use std::collections::BTreeMap;
    use tempfile::TempDir;

    fn repo() -> TempDir {
        let tmp = TempDir::new().unwrap();
        init(tmp.path(), "https://github.com/edger-dev/kinora").unwrap();
        tmp
    }

    fn params(content: &[u8], name: &str) -> StoreKinoParams {
        StoreKinoParams {
            kind: "markdown".into(),
            content: content.to_vec(),
            author: "yj".into(),
            provenance: "test".into(),
            ts: "2026-04-18T10:00:00Z".into(),
            metadata: BTreeMap::from([("name".into(), name.into())]),
            id: None,
            parents: vec![],
        }
    }

    #[test]
    fn render_writes_pages_under_override_cache_dir() {
        let tmp = repo();
        let kin = kinora_root(tmp.path());
        store_kino(&kin, params(b"# hello\n", "greet")).unwrap();

        let cache = TempDir::new().unwrap();
        let args = RenderRunArgs {
            cache_dir: Some(cache.path().to_string_lossy().into_owned()),
        };
        let report = run_render(tmp.path(), args).unwrap();
        assert_eq!(report.cache_path, cache.path());
        assert_eq!(report.page_count, 1);
        assert!(cache.path().join("book.toml").is_file());
        assert!(cache.path().join("src/SUMMARY.md").is_file());
    }

    #[test]
    fn render_errors_when_run_outside_kinora_repo() {
        let tmp = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let args = RenderRunArgs {
            cache_dir: Some(cache.path().to_string_lossy().into_owned()),
        };
        let err = run_render(tmp.path(), args).unwrap_err();
        assert!(matches!(err, CliError::NotInKinoraRepo { .. }));
    }

    #[test]
    fn render_over_existing_output_overwrites() {
        let tmp = repo();
        let kin = kinora_root(tmp.path());
        store_kino(&kin, params(b"v1", "doc")).unwrap();
        let cache = TempDir::new().unwrap();

        let args = RenderRunArgs {
            cache_dir: Some(cache.path().to_string_lossy().into_owned()),
        };
        run_render(tmp.path(), args).unwrap();

        let stale = cache.path().join("src").join("stale.md");
        std::fs::write(&stale, "stale").unwrap();
        assert!(stale.exists());

        let args = RenderRunArgs {
            cache_dir: Some(cache.path().to_string_lossy().into_owned()),
        };
        run_render(tmp.path(), args).unwrap();
        assert!(!stale.exists());
    }

    #[test]
    fn render_empty_repo_produces_empty_book() {
        let tmp = repo();
        let cache = TempDir::new().unwrap();
        let args = RenderRunArgs {
            cache_dir: Some(cache.path().to_string_lossy().into_owned()),
        };
        let report = run_render(tmp.path(), args).unwrap();
        assert_eq!(report.page_count, 0);
        let summary =
            std::fs::read_to_string(cache.path().join("src/SUMMARY.md")).unwrap();
        assert!(summary.starts_with("# Summary"));
    }
}
