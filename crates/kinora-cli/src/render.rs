use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use kinora::cache_path::CachePath;
use kinora::commit::read_root_pointer;
use kinora::config::Config;
use kinora::git_state::{
    ExtractError, WorktreeInfo, extract_subtree, list_local_branches, list_worktrees,
};
use kinora::paths::{config_path, kinora_root, roots_dir};
use kinora::render::{Book, render, write_book};
use kinora::resolve::Resolver;
use kinora::root::RootKinograph;
use kinora::store::ContentStore;
use tempfile::TempDir;

use crate::common::{CliError, find_repo_root};

pub struct RenderRunArgs {
    pub cache_dir: Option<String>,
}

#[derive(Debug)]
pub struct RenderReport {
    pub cache_path: PathBuf,
    pub page_count: usize,
    pub skipped_count: usize,
    /// Number of distinct branch/worktree sources that produced pages.
    /// `0` means the render fell back to the working-copy layout — either
    /// because the repo isn't a git repo or no branch has `.kinora/` yet.
    pub source_count: usize,
}

pub fn run_render(cwd: &Path, args: RenderRunArgs) -> Result<RenderReport, CliError> {
    let repo_root = find_repo_root(cwd)?;
    let kin_root = kinora_root(&repo_root);

    // Config read from the working copy — it's the authoritative repo-url
    // for cache-path derivation, regardless of per-branch state.
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

    // Try the git-tree-based render first; fall back to working-copy when
    // the repo isn't a git repo or no branch carries `.kinora/` yet.
    // `_tmps` keeps scratch dirs for blob materialization alive through
    // the render — dropped after `write_book` returns.
    let outcome = match render_from_git(&repo_root)? {
        Some(o) => o,
        None => render_from_working_copy(&kin_root)?,
    };
    let RenderOutcome { book, source_count, _tmps } = outcome;

    let page_count = book.pages.len();
    let skipped_count = book.skipped.len();

    let title = if cache.name.is_empty() {
        "kinora".to_owned()
    } else {
        cache.name.clone()
    };
    write_book(&cache_path, &title, &book)?;

    Ok(RenderReport {
        cache_path,
        page_count,
        skipped_count,
        source_count,
    })
}

/// Carries the rendered Book plus the scratch tempdirs whose contents
/// back its kinos — the tempdirs must outlive the render so file reads
/// resolve, but the caller only cares about the Book afterwards.
struct RenderOutcome {
    book: Book,
    source_count: usize,
    _tmps: Vec<TempDir>,
}

/// Render each local branch and linked worktree from its committed
/// `.kinora/` tree. Returns `Ok(None)` if the repo isn't a git repo or no
/// source has `.kinora/` — caller falls back to the working-copy render.
fn render_from_git(repo_root: &Path) -> Result<Option<RenderOutcome>, CliError> {
    let repo = match gix::open(repo_root) {
        Ok(r) => r,
        Err(_) => return Ok(None),
    };

    let branches = list_local_branches(&repo)?;
    let worktrees = list_worktrees(&repo)?;
    let sources = combine_sources(branches, &worktrees);

    let mut book = Book::default();
    let mut tmps = Vec::new();
    let mut source_count = 0;

    for source in sources {
        let td = TempDir::new()?;
        match extract_subtree(&repo, source.commit, ".kinora", td.path()) {
            Ok(()) => {}
            // A branch existed before kinora was introduced — legitimate,
            // just skip it rather than aborting the whole render.
            Err(ExtractError::SubtreeAbsent { .. }) => continue,
            Err(e) => return Err(e.into()),
        }

        let resolver = Resolver::load(td.path())?;
        // Every page renders under `source.label` (branch or worktree
        // name). Root-level grouping ("main"/"unreferenced") is dropped
        // for the git path: branches *are* the top-level grouping now.
        let per_source = render(&resolver, &HashMap::new(), &source.label)?;
        book.pages.extend(per_source.pages);
        book.skipped.extend(per_source.skipped);
        source_count += 1;
        tmps.push(td);
    }

    if source_count == 0 {
        return Ok(None);
    }
    Ok(Some(RenderOutcome {
        book,
        source_count,
        _tmps: tmps,
    }))
}

/// Fallback: read `.kinora/` from the working directory with the legacy
/// root-based grouping. Used when git isn't available or no branch has
/// `.kinora/` committed yet (e.g. a freshly-initialized repo).
fn render_from_working_copy(kin_root: &Path) -> Result<RenderOutcome, CliError> {
    let resolver = Resolver::load(kin_root)?;
    let owners = build_owners_map(kin_root)?;
    let book = render(&resolver, &owners, "unreferenced")?;
    Ok(RenderOutcome {
        book,
        source_count: 0,
        _tmps: Vec::new(),
    })
}

/// Merge branches + worktrees into one ordered source list, deduping
/// worktrees whose ref is already represented by a local branch. Branches
/// take precedence (they're named simply after the branch; worktree labels
/// can include transient worktree ids).
struct RenderSource {
    label: String,
    commit: gix::ObjectId,
}

fn combine_sources(
    branches: Vec<(String, gix::ObjectId)>,
    worktrees: &[WorktreeInfo],
) -> Vec<RenderSource> {
    let mut seen_refs: std::collections::BTreeSet<String> = branches
        .iter()
        .map(|(n, _)| format!("refs/heads/{n}"))
        .collect();
    let mut sources: Vec<RenderSource> = branches
        .into_iter()
        .map(|(label, commit)| RenderSource { label, commit })
        .collect();
    for wt in worktrees {
        match &wt.ref_name {
            Some(r) if seen_refs.contains(r) => continue,
            Some(r) => {
                seen_refs.insert(r.clone());
                sources.push(RenderSource {
                    label: format!("worktree-{}", wt.label),
                    commit: wt.head_commit,
                });
            }
            None => {
                sources.push(RenderSource {
                    label: format!("worktree-{}", wt.label),
                    commit: wt.head_commit,
                });
            }
        }
    }
    // Sort for deterministic output across runs.
    sources.sort_by(|a, b| a.label.cmp(&b.label));
    sources
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
    use kinora::commit::{CommitParams, commit_root};
    use kinora::init::init;
    use kinora::kino::{StoreKinoParams, store_kino};
    use std::collections::BTreeMap;
    use std::process::Command;
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
        // keep-last-10 rather than Never: under kinora-bayr the Never
        // policy prunes owned store events from staging after commit, and
        // the current Resolver only looks at staging — so render would
        // see no identities for the committed kinos. These tests are about
        // root grouping semantics, not prune policy.
        std::fs::write(
            kinora::paths::config_path(kin),
            "repo-url \"https://github.com/edger-dev/kinora\"\nroots {\n  main { policy \"keep-last-10\" }\n}\n",
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

    // ------------------------------------------------------------------
    // run_render: single-source (non-git) fallback paths
    // ------------------------------------------------------------------

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
        assert_eq!(report.source_count, 0, "non-git repo should fall back");
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
    // End-to-end render grouping — fallback (non-git) path
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

    // ------------------------------------------------------------------
    // Multi-branch end-to-end (git path)
    // ------------------------------------------------------------------

    fn git(args: &[&str], cwd: &Path) {
        let out = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .env_remove("GIT_CONFIG_GLOBAL")
            .env("HOME", cwd)
            .output()
            .expect("spawn git");
        assert!(
            out.status.success(),
            "git {args:?} failed: stdout={:?} stderr={:?}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
    }

    fn git_init(path: &Path) {
        git(&["init", "-b", "main"], path);
        git(&["config", "user.name", "test"], path);
        git(&["config", "user.email", "test@example.com"], path);
    }

    #[test]
    fn multi_branch_render_produces_one_section_per_branch() {
        let tmp = TempDir::new().unwrap();
        git_init(tmp.path());
        init(tmp.path(), "https://github.com/edger-dev/kinora").unwrap();
        let kin = kinora_root(tmp.path());

        // main branch: kino "alpha"
        store_kino(&kin, params(b"# alpha\n", "alpha")).unwrap();
        git(&["add", "-A"], tmp.path());
        git(&["commit", "-m", "seed main"], tmp.path());

        // feature-x branch: kino "beta" (alpha is still present because it
        // was committed before branching)
        git(&["checkout", "-b", "feature-x"], tmp.path());
        store_kino(&kin, params(b"# beta\n", "beta")).unwrap();
        git(&["add", "-A"], tmp.path());
        git(&["commit", "-m", "add beta on feature-x"], tmp.path());

        let cache = TempDir::new().unwrap();
        let args = RenderRunArgs {
            cache_dir: Some(cache.path().to_string_lossy().into_owned()),
        };
        let report = run_render(tmp.path(), args).unwrap();
        assert_eq!(report.source_count, 2, "two branches: main + feature-x");

        let summary =
            std::fs::read_to_string(cache.path().join("src/SUMMARY.md")).unwrap();
        assert!(summary.contains("[main]"), "summary missing main: {summary}");
        assert!(
            summary.contains("[feature-x]"),
            "summary missing feature-x: {summary}"
        );

        // A kino that exists only on feature-x appears only under feature-x.
        assert!(cache.path().join("src/feature-x").is_dir());
        assert!(cache.path().join("src/main").is_dir());

        // The alpha kino — committed on main before branching — should be
        // present on BOTH branches (one page per (branch, kino) pair).
        let feature_x_files: Vec<String> =
            std::fs::read_dir(cache.path().join("src/feature-x"))
                .unwrap()
                .filter_map(|e| e.ok())
                .filter_map(|e| e.file_name().to_str().map(str::to_owned))
                .collect();
        assert!(
            feature_x_files.iter().any(|n| n.starts_with("alpha-")),
            "alpha missing on feature-x: {feature_x_files:?}",
        );
        assert!(
            feature_x_files.iter().any(|n| n.starts_with("beta-")),
            "beta missing on feature-x: {feature_x_files:?}",
        );

        let main_files: Vec<String> = std::fs::read_dir(cache.path().join("src/main"))
            .unwrap()
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().to_str().map(str::to_owned))
            .collect();
        assert!(
            main_files.iter().any(|n| n.starts_with("alpha-")),
            "alpha missing on main: {main_files:?}",
        );
        assert!(
            !main_files.iter().any(|n| n.starts_with("beta-")),
            "beta must not appear on main: {main_files:?}",
        );
    }

    #[test]
    fn source_marker_cites_originating_branch() {
        let tmp = TempDir::new().unwrap();
        git_init(tmp.path());
        init(tmp.path(), "https://github.com/edger-dev/kinora").unwrap();
        let kin = kinora_root(tmp.path());

        store_kino(&kin, params(b"# hello\n", "greet")).unwrap();
        git(&["add", "-A"], tmp.path());
        git(&["commit", "-m", "seed"], tmp.path());

        let cache = TempDir::new().unwrap();
        let args = RenderRunArgs {
            cache_dir: Some(cache.path().to_string_lossy().into_owned()),
        };
        run_render(tmp.path(), args).unwrap();

        // Find the single page and read it.
        let main_dir = cache.path().join("src/main");
        let entries: Vec<PathBuf> = std::fs::read_dir(&main_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("md"))
            .filter(|p| p.file_stem().and_then(|s| s.to_str()) != Some("index"))
            .collect();
        assert_eq!(entries.len(), 1, "expected 1 page on main, got {entries:?}");
        let body = std::fs::read_to_string(&entries[0]).unwrap();
        assert!(
            body.contains("Rendered from `main`"),
            "source marker missing/wrong: {body}"
        );
    }

    #[test]
    fn render_falls_back_when_git_has_no_kinora() {
        // Git repo exists but `.kinora/` is only in the working dir (never
        // committed). The render should fall back to the working-copy path
        // so users pre-first-commit still see their kinos.
        let tmp = TempDir::new().unwrap();
        git_init(tmp.path());
        init(tmp.path(), "https://github.com/edger-dev/kinora").unwrap();
        let kin = kinora_root(tmp.path());

        // Make a commit that doesn't include .kinora/
        std::fs::write(tmp.path().join("README.md"), b"# readme\n").unwrap();
        git(&["add", "README.md"], tmp.path());
        git(&["commit", "-m", "readme only"], tmp.path());

        store_kino(&kin, params(b"# hello\n", "greet")).unwrap();

        let cache = TempDir::new().unwrap();
        let args = RenderRunArgs {
            cache_dir: Some(cache.path().to_string_lossy().into_owned()),
        };
        let report = run_render(tmp.path(), args).unwrap();
        assert_eq!(report.page_count, 1);
        assert_eq!(
            report.source_count, 0,
            "no branch has .kinora/, expected fallback"
        );
        assert!(cache.path().join("src/unreferenced").is_dir());
    }

    #[test]
    fn branch_without_kinora_is_skipped_not_aborted() {
        // One branch has `.kinora/` committed, another doesn't (legitimate
        // for a branch that existed before kinora was introduced). The
        // render should skip the second branch via `ExtractError::SubtreeAbsent`
        // rather than falling back or failing the whole render.
        let tmp = TempDir::new().unwrap();
        git_init(tmp.path());

        // Seed a pre-kinora commit on main so the branch `legacy` can
        // point at it without ever having seen `.kinora/`.
        std::fs::write(tmp.path().join("README.md"), b"# readme\n").unwrap();
        git(&["add", "README.md"], tmp.path());
        git(&["commit", "-m", "pre-kinora"], tmp.path());
        git(&["branch", "legacy"], tmp.path());

        init(tmp.path(), "https://github.com/edger-dev/kinora").unwrap();
        let kin = kinora_root(tmp.path());
        store_kino(&kin, params(b"# alpha\n", "alpha")).unwrap();
        git(&["add", "-A"], tmp.path());
        git(&["commit", "-m", "add kinora on main"], tmp.path());

        let cache = TempDir::new().unwrap();
        let args = RenderRunArgs {
            cache_dir: Some(cache.path().to_string_lossy().into_owned()),
        };
        let report = run_render(tmp.path(), args).unwrap();
        // `legacy` had no .kinora/ — skipped. Only `main` contributes.
        assert_eq!(report.source_count, 1, "got {report:?}");
        assert!(cache.path().join("src/main").is_dir());
        assert!(!cache.path().join("src/legacy").exists());
    }

    #[test]
    fn detached_head_worktree_surfaces_as_its_own_group() {
        // A linked worktree checked out in detached-HEAD state has no ref
        // to dedupe against — it should surface under its own `worktree-*`
        // label.
        let tmp = TempDir::new().unwrap();
        git_init(tmp.path());
        init(tmp.path(), "https://github.com/edger-dev/kinora").unwrap();
        let kin = kinora_root(tmp.path());

        store_kino(&kin, params(b"# alpha\n", "alpha")).unwrap();
        git(&["add", "-A"], tmp.path());
        git(&["commit", "-m", "seed"], tmp.path());

        let wt_parent = TempDir::new().unwrap();
        let wt_path = wt_parent.path().join("detached");
        git(
            &["worktree", "add", "--detach", wt_path.to_str().unwrap(), "HEAD"],
            tmp.path(),
        );

        let cache = TempDir::new().unwrap();
        let args = RenderRunArgs {
            cache_dir: Some(cache.path().to_string_lossy().into_owned()),
        };
        let report = run_render(tmp.path(), args).unwrap();
        // main (branch) + detached worktree = 2 sources.
        assert_eq!(report.source_count, 2, "got {report:?}");
        let dirs: Vec<String> = std::fs::read_dir(cache.path().join("src"))
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .filter_map(|e| e.file_name().to_str().map(str::to_owned))
            .collect();
        assert!(
            dirs.iter().any(|f| f.starts_with("worktree-")),
            "expected a worktree-* group for detached HEAD: {dirs:?}",
        );
    }

    #[test]
    fn worktree_dedupes_against_matching_branch() {
        let tmp = TempDir::new().unwrap();
        git_init(tmp.path());
        init(tmp.path(), "https://github.com/edger-dev/kinora").unwrap();
        let kin = kinora_root(tmp.path());

        store_kino(&kin, params(b"# alpha\n", "alpha")).unwrap();
        git(&["add", "-A"], tmp.path());
        git(&["commit", "-m", "seed"], tmp.path());

        // Create a linked worktree on a new branch. The branch is fully
        // enumerated through the normal refs/heads path — the worktree is
        // just a view onto it — so the render should dedupe and NOT
        // produce a separate "worktree-*" section.
        let wt_parent = TempDir::new().unwrap();
        let wt_path = wt_parent.path().join("extra");
        git(
            &[
                "worktree",
                "add",
                "-b",
                "wt-branch",
                wt_path.to_str().unwrap(),
            ],
            tmp.path(),
        );

        let cache = TempDir::new().unwrap();
        let args = RenderRunArgs {
            cache_dir: Some(cache.path().to_string_lossy().into_owned()),
        };
        let report = run_render(tmp.path(), args).unwrap();
        // Two branches (main, wt-branch), one linked worktree deduped
        // against wt-branch → two sources.
        assert_eq!(report.source_count, 2, "got {report:?}");
        assert!(cache.path().join("src/main").is_dir());
        assert!(cache.path().join("src/wt-branch").is_dir());
        let files: Vec<String> = std::fs::read_dir(cache.path().join("src"))
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .filter_map(|e| e.file_name().to_str().map(str::to_owned))
            .collect();
        assert!(
            !files.iter().any(|f| f.starts_with("worktree-")),
            "worktree should dedupe against refs/heads/wt-branch: {files:?}",
        );
    }
}
