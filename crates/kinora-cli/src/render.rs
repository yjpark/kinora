use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use kinora::cache_path::CachePath;
use kinora::commit::read_root_pointer;
use kinora::config::Config;
use kinora::paths::{config_path, kinora_root, roots_dir};
use kinora::render::{render, write_book};
use kinora::resolve::Resolver;
use kinora::root::RootKinograph;
use kinora::store::ContentStore;

use crate::common::{find_repo_root, CliError};

pub struct RenderRunArgs {
    pub cache_dir: Option<String>,
}

#[derive(Debug)]
pub struct RenderReport {
    pub cache_path: PathBuf,
    pub page_count: usize,
    pub skipped_count: usize,
}

pub fn run_render(cwd: &Path, args: RenderRunArgs) -> Result<RenderReport, CliError> {
    let repo_root = find_repo_root(cwd)?;
    let kin_root = kinora_root(&repo_root);

    let config_text = std::fs::read_to_string(config_path(&kin_root))?;
    let config = Config::from_styx(&config_text)?;

    let cache = CachePath::from_repo_url(&config.repo_url);
    let cache_path = match args.cache_dir {
        Some(override_dir) => PathBuf::from(override_dir),
        None => {
            let xdg = std::env::var("XDG_CACHE_HOME").ok();
            let home = std::env::var("HOME").ok();
            resolve_cache_root(xdg.as_deref(), home.as_deref())?.join(cache.subdir())
        }
    };

    let resolver = Resolver::load(&kin_root)?;
    let owners = build_owners_map(&kin_root)?;
    let book = render(&resolver, &owners, "unreferenced")?;
    let page_count = book.pages.len();
    let skipped_count = book.skipped.len();

    let title = if cache.name.is_empty() {
        "kinora".to_owned()
    } else {
        cache.name.clone()
    };
    write_book(&cache_path, &title, &book)?;

    Ok(RenderReport { cache_path, page_count, skipped_count })
}

/// Pure resolver so the XDG/HOME branching can be unit-tested without
/// touching process env.
fn resolve_cache_root(xdg: Option<&str>, home: Option<&str>) -> Result<PathBuf, CliError> {
    if let Some(x) = xdg
        && !x.is_empty()
    {
        return Ok(PathBuf::from(x).join("kinora"));
    }
    if let Some(h) = home
        && !h.is_empty()
    {
        return Ok(PathBuf::from(h).join(".cache").join("kinora"));
    }
    Err(CliError::CacheHomeUnresolved)
}

/// Build a map from kino id to the name of the root that owns it.
///
/// Scans `.kinora/roots/` pointer files; for each pointer, loads the
/// referenced root kinograph blob and records every entry id under that
/// root's name. Kinos that are not owned by any committed root are left
/// out — callers should fall back to a default label for them.
fn build_owners_map(kin_root: &Path) -> Result<HashMap<String, String>, CliError> {
    let mut owners: HashMap<String, String> = HashMap::new();
    let dir = roots_dir(kin_root);
    let entries = match std::fs::read_dir(&dir) {
        Ok(it) => it,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(owners),
        Err(e) => return Err(CliError::Io(e)),
    };

    // Collect + sort pointer names so multi-root insertion order is
    // deterministic. Post-phase-3 ownership is exclusive, so collisions
    // shouldn't happen — but until then, a stable "last writer wins"
    // rule keeps render output reproducible across machines.
    let mut names: Vec<String> = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let file_name = entry.file_name();
        let Some(root_name) = file_name.to_str() else {
            continue;
        };
        // Skip `.<name>.tmp` files `write_root_pointer` creates mid-rename.
        if root_name.starts_with('.') {
            continue;
        }
        // The `commits` root holds per-commit archive kinos — infrastructure
        // metadata, not user content. Render consumers (`mdbook` pages)
        // should not see them; they're surfaced through `kinora log` / a
        // future dedicated command instead.
        if root_name == "commits" {
            continue;
        }
        names.push(root_name.to_owned());
    }
    names.sort();

    let store = ContentStore::new(kin_root);
    for root_name in names {
        let Some(hash) = read_root_pointer(kin_root, &root_name)? else {
            continue;
        };
        let bytes = store.read(&hash).map_err(CliError::Store)?;
        let kinograph = RootKinograph::parse(&bytes).map_err(CliError::Root)?;
        for kino in kinograph.entries {
            owners.insert(kino.id, root_name.clone());
        }
    }
    Ok(owners)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kinora::commit::{commit_root, CommitParams};
    use kinora::init::init;
    use kinora::kino::{store_kino, StoreKinoParams};
    use std::collections::BTreeMap;
    use tempfile::TempDir;

    fn repo() -> TempDir {
        let tmp = TempDir::new().unwrap();
        init(tmp.path(), "https://github.com/edger-dev/kinora").unwrap();
        tmp
    }

    fn commit_params() -> CommitParams {
        CommitParams {
            author: "yj".into(),
            provenance: "test".into(),
            ts: "2026-04-19T10:00:00Z".into(),
        }
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

    /// Overwrite `.kinora/config.styx` to declare `main` as a root.
    /// Needed for tests that route kinos into `main` via explicit assigns —
    /// the default config only auto-provisions `inbox`.
    fn declare_main_root(kin: &std::path::Path) {
        std::fs::write(
            kinora::paths::config_path(kin),
            "repo-url \"https://github.com/edger-dev/kinora\"\nroots {\n  main { policy \"never\" }\n}\n",
        )
        .unwrap();
    }

    fn assign_to(kin: &std::path::Path, kino_id: &str, target_root: &str) {
        kinora::assign::write_assign(
            kin,
            &kinora::assign::AssignEvent {
                kino_id: kino_id.to_owned(),
                target_root: target_root.to_owned(),
                supersedes: vec![],
                author: "yj".into(),
                ts: "2026-04-18T10:00:01Z".into(),
                provenance: "test".into(),
            },
        )
        .unwrap();
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
    fn resolve_cache_root_prefers_xdg_over_home() {
        let got = resolve_cache_root(Some("/xdg"), Some("/home")).unwrap();
        assert_eq!(got, PathBuf::from("/xdg/kinora"));
    }

    #[test]
    fn resolve_cache_root_falls_back_to_home_when_xdg_absent() {
        assert_eq!(
            resolve_cache_root(None, Some("/home/user")).unwrap(),
            PathBuf::from("/home/user/.cache/kinora"),
        );
    }

    #[test]
    fn resolve_cache_root_ignores_empty_env_values() {
        assert_eq!(
            resolve_cache_root(Some(""), Some("/home/user")).unwrap(),
            PathBuf::from("/home/user/.cache/kinora"),
        );
    }

    #[test]
    fn resolve_cache_root_errors_when_nothing_resolves() {
        let err = resolve_cache_root(None, None).unwrap_err();
        assert!(matches!(err, CliError::CacheHomeUnresolved));
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

    // ------------------------------------------------------------------
    // build_owners_map
    // ------------------------------------------------------------------

    #[test]
    fn build_owners_map_empty_when_no_roots_dir() {
        let tmp = repo();
        let kin = kinora_root(tmp.path());
        let owners = build_owners_map(&kin).unwrap();
        assert!(owners.is_empty(), "expected empty map, got: {owners:?}");
    }

    #[test]
    fn build_owners_map_ignores_tmp_and_non_file_entries() {
        let tmp = repo();
        let kin = kinora_root(tmp.path());
        declare_main_root(&kin);
        let ev = store_kino(&kin, params(b"alpha", "alpha")).unwrap();
        assign_to(&kin, &ev.event.id, "main");
        commit_root(&kin, "main", commit_params()).unwrap();

        // Simulate a leftover tmp pointer and a stray subdir under roots/.
        let roots = kin.join("roots");
        std::fs::write(roots.join(".main.tmp"), "garbage").unwrap();
        std::fs::create_dir(roots.join("nested-dir")).unwrap();

        // Should still succeed and return the one real root.
        let owners = build_owners_map(&kin).unwrap();
        assert!(owners.values().any(|v| v == "main"));
    }

    #[test]
    fn build_owners_map_maps_entries_to_root_name() {
        let tmp = repo();
        let kin = kinora_root(tmp.path());
        declare_main_root(&kin);

        let ev1 = store_kino(&kin, params(b"alpha", "alpha")).unwrap();
        let ev2 = store_kino(&kin, params(b"beta", "beta")).unwrap();
        assign_to(&kin, &ev1.event.id, "main");
        assign_to(&kin, &ev2.event.id, "main");

        commit_root(&kin, "main", commit_params()).unwrap();

        let owners = build_owners_map(&kin).unwrap();
        assert_eq!(owners.get(&ev1.event.id).map(String::as_str), Some("main"));
        assert_eq!(owners.get(&ev2.event.id).map(String::as_str), Some("main"));
    }

    #[test]
    fn build_owners_map_skips_commits_root_even_with_stale_pointer() {
        // The `commits` root is infrastructure — per-commit archive kinos,
        // not user content. render must skip it regardless of pointer state.
        // This test seeds `roots/commits` with a pointer to a non-existent
        // hash; if the filter weren't in place, build_owners_map would try
        // to read the blob and fail with a store error.
        let tmp = repo();
        let kin = kinora_root(tmp.path());
        let roots = kin.join("roots");
        std::fs::create_dir_all(&roots).unwrap();
        std::fs::write(roots.join("commits"), "ff".repeat(32)).unwrap();

        // No error, and nothing maps to "commits".
        let owners = build_owners_map(&kin).unwrap();
        assert!(
            owners.values().all(|r| r != "commits"),
            "owners must not map anything to commits: {owners:?}",
        );
    }

    // ------------------------------------------------------------------
    // End-to-end render grouping
    // ------------------------------------------------------------------

    #[test]
    fn render_pure_staged_repo_groups_under_unreferenced() {
        let tmp = repo();
        let kin = kinora_root(tmp.path());
        store_kino(&kin, params(b"# a\n", "alpha")).unwrap();
        store_kino(&kin, params(b"# b\n", "beta")).unwrap();

        let cache = TempDir::new().unwrap();
        let args = RenderRunArgs {
            cache_dir: Some(cache.path().to_string_lossy().into_owned()),
        };
        let report = run_render(tmp.path(), args).unwrap();
        assert_eq!(report.page_count, 2);
        assert!(cache.path().join("src/unreferenced/index.md").is_file());
        assert!(!cache.path().join("src/main").exists());
    }

    #[test]
    fn render_committed_main_groups_under_main() {
        let tmp = repo();
        let kin = kinora_root(tmp.path());
        declare_main_root(&kin);
        let a = store_kino(&kin, params(b"# a\n", "alpha")).unwrap();
        let b = store_kino(&kin, params(b"# b\n", "beta")).unwrap();
        assign_to(&kin, &a.event.id, "main");
        assign_to(&kin, &b.event.id, "main");
        commit_root(&kin, "main", commit_params()).unwrap();

        let cache = TempDir::new().unwrap();
        let args = RenderRunArgs {
            cache_dir: Some(cache.path().to_string_lossy().into_owned()),
        };
        let report = run_render(tmp.path(), args).unwrap();
        assert_eq!(report.page_count, 2);
        assert!(cache.path().join("src/main/index.md").is_file());
        assert!(!cache.path().join("src/unreferenced").exists());
    }

    #[test]
    fn render_mixed_repo_splits_between_main_and_unreferenced() {
        let tmp = repo();
        let kin = kinora_root(tmp.path());
        declare_main_root(&kin);
        let a = store_kino(&kin, params(b"# a\n", "alpha")).unwrap();
        let b = store_kino(&kin, params(b"# b\n", "beta")).unwrap();
        assign_to(&kin, &a.event.id, "main");
        assign_to(&kin, &b.event.id, "main");
        commit_root(&kin, "main", commit_params()).unwrap();

        // Add a post-commit kino that isn't owned by any root yet.
        store_kino(&kin, params(b"# c\n", "gamma")).unwrap();

        let cache = TempDir::new().unwrap();
        let args = RenderRunArgs {
            cache_dir: Some(cache.path().to_string_lossy().into_owned()),
        };
        let report = run_render(tmp.path(), args).unwrap();
        assert_eq!(report.page_count, 3);
        assert!(cache.path().join("src/main/index.md").is_file());
        assert!(cache.path().join("src/unreferenced/index.md").is_file());
    }
}
