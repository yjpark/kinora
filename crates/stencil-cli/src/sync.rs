//! `stencil sync <paths>`: scan files for stencil markers, render the bound
//! api-kinograph through the engine, and write changed files atomically.
//!
//! This module is the file-I/O and path-walking shell around the pure
//! [`stencil::engine`]. The engine decides *what* a file should contain; this
//! module finds the files, reads them, writes the changed ones back atomically,
//! and tallies a [`SyncSummary`] for display and exit-code selection.
//!
//! Errors are split by blast radius: repo-level setup problems (resolver load
//! failure, a path argument that doesn't exist) are fatal and abort the run;
//! per-file problems (parse failure, a slot whose entry can't be resolved) are
//! collected into the summary so one bad file never hides the rest.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use kinora::paths::kinora_root;
use kinora::resolve::Resolver;
use stencil::engine::{sync_file, SlotStatus, SyncReport};
use stencil::region::StencilFile;
use stencil::target::LanguageTarget;
use stencil::StencilError;

use crate::common::CliError;

/// What happened to a single file during a sync run.
#[derive(Debug)]
pub struct FileOutcome {
    /// The file's path (as resolved against the cwd).
    pub path: PathBuf,
    /// The engine report, or the [`StencilError`] that stopped this file
    /// (read/parse/resolve). A per-file error never aborts the whole run.
    pub result: Result<SyncReport, StencilError>,
    /// Whether the file was rewritten on disk.
    pub changed: bool,
}

/// The result of a whole `stencil sync` run: one [`FileOutcome`] per file
/// scanned, in deterministic path order.
#[derive(Debug)]
pub struct SyncSummary {
    pub files: Vec<FileOutcome>,
}

impl SyncSummary {
    /// Number of files rewritten.
    pub fn files_changed(&self) -> usize {
        self.files.iter().filter(|f| f.changed).count()
    }

    /// Whether the run hit any condition that should force a non-zero exit: a
    /// per-file hard error, or a slot naming no kinograph entry (unknown slot).
    pub fn has_errors(&self) -> bool {
        self.files.iter().any(|f| match &f.result {
            Err(_) => true,
            Ok(report) => !report.unmatched().is_empty(),
        })
    }
}

/// Sync every stencil file reachable from `paths` (resolved against `cwd`)
/// against the api-kinograph bindings they declare, using `repo_root` to load
/// the kinora [`Resolver`].
///
/// `paths` may name files or directories; an empty list defaults to `cwd`.
/// Directories are walked recursively for `*.rs` files (the only language the
/// shipped [`stencil::target::RustTarget`] renders), skipping `target/` and
/// dot-directories. A path argument that does not exist is a fatal error.
pub fn run_sync(
    repo_root: &Path,
    cwd: &Path,
    paths: &[String],
    target: &dyn LanguageTarget,
) -> Result<SyncSummary, CliError> {
    let resolver = Resolver::load(kinora_root(repo_root))?;
    let files = collect_files(cwd, paths)?;
    let outcomes = files.iter().map(|p| sync_one(p, &resolver, target)).collect();
    Ok(SyncSummary { files: outcomes })
}

/// Resolve `paths` against `cwd` into a deduplicated, sorted list of files to
/// sync. An empty `paths` defaults to `cwd`. File arguments are taken verbatim;
/// directory arguments are walked recursively. A path that does not exist is a
/// fatal [`CliError::PathNotFound`].
fn collect_files(cwd: &Path, paths: &[String]) -> Result<Vec<PathBuf>, CliError> {
    let default = [".".to_string()];
    let paths = if paths.is_empty() { &default[..] } else { paths };

    // Dedup by *canonical* identity (so `sub/a.rs` and `./sub/a.rs` count once)
    // while keeping the original spelling for display.
    let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
    let mut out: Vec<PathBuf> = Vec::new();
    for p in paths {
        let path = cwd.join(p);
        if path.is_file() {
            push_unique(path, &mut seen, &mut out);
        } else if path.is_dir() {
            walk_dir(&path, &mut seen, &mut out)?;
        } else {
            return Err(CliError::PathNotFound { path });
        }
    }
    out.sort();
    Ok(out)
}

/// Add `path` to `out` unless a path with the same canonical identity is
/// already present. Canonicalization can fail (e.g. on a broken symlink); fall
/// back to the literal path so the file is still considered.
fn push_unique(path: PathBuf, seen: &mut BTreeSet<PathBuf>, out: &mut Vec<PathBuf>) {
    let key = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
    if seen.insert(key) {
        out.push(path);
    }
}

/// Recursively collect `*.rs` files under `dir`, skipping dot-directories and
/// `target/`. `.rs` is the only extension the shipped [`RustTarget`] renders;
/// when other language targets land, this generalizes to their extensions.
///
/// Symlinks are not followed (`file_type` is queried without dereferencing):
/// a symlinked file or directory is skipped, which keeps the walk free of
/// symlink cycles at the cost of not discovering link-only sources.
///
/// [`RustTarget`]: stencil::target::RustTarget
fn walk_dir(dir: &Path, seen: &mut BTreeSet<PathBuf>, out: &mut Vec<PathBuf>) -> Result<(), CliError> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if file_type.is_dir() {
            if name.starts_with('.') || name == "target" {
                continue;
            }
            walk_dir(&entry.path(), seen, out)?;
        } else if file_type.is_file() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                push_unique(path, seen, out);
            }
        }
    }
    Ok(())
}

/// Sync one file: read, parse, render, and (if changed) write it back. Any
/// [`StencilError`] is captured in the returned [`FileOutcome`] rather than
/// propagated, so one bad file never aborts the run.
fn sync_one(path: &Path, resolver: &Resolver, target: &dyn LanguageTarget) -> FileOutcome {
    match render_to_disk(path, resolver, target) {
        Ok((report, changed)) => FileOutcome { path: path.to_path_buf(), result: Ok(report), changed },
        Err(e) => FileOutcome { path: path.to_path_buf(), result: Err(e), changed: false },
    }
}

/// Read, parse, sync, and write one file. Returns the engine report and whether
/// the file was rewritten.
fn render_to_disk(
    path: &Path,
    resolver: &Resolver,
    target: &dyn LanguageTarget,
) -> Result<(SyncReport, bool), StencilError> {
    let src = fs::read_to_string(path)?;
    let parsed = StencilFile::parse(&src, target)?;
    let outcome = sync_file(&parsed, resolver, target)?;
    let changed = outcome.report.changed();
    if changed {
        atomic_write(path, outcome.file.to_source(target).as_bytes())?;
    }
    Ok((outcome.report, changed))
}

/// Write `bytes` to `path` atomically: stage to a sibling temp file, then
/// rename over the target. Mirrors kinora's `Store::write`.
fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    let stem = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "out".into());
    let tmp = dir.join(format!(".stencil-tmp-{stem}"));
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Render a human-readable summary of a sync run: a header, then one line per
/// noteworthy file with its slot tallies, plus drift warnings, unmatched-slot
/// errors, unslotted-entry warnings, and per-file hard errors.
pub fn format_sync_summary(summary: &SyncSummary) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "scanned {} file(s), {} changed",
        summary.files.len(),
        summary.files_changed()
    );

    for f in &summary.files {
        let name = f.path.display();
        let report = match &f.result {
            Err(e) => {
                let _ = writeln!(out, "  {name}: error: {e}");
                continue;
            }
            Ok(report) => report,
        };

        let mut created = 0u32;
        let mut updated = 0u32;
        let mut drift = 0u32;
        for s in &report.slots {
            match s.status {
                SlotStatus::Created => created += 1,
                SlotStatus::Updated => updated += 1,
                SlotStatus::DriftOverwritten => drift += 1,
                SlotStatus::Unchanged | SlotStatus::Unmatched => {}
            }
        }

        let noteworthy = f.changed
            || !report.unmatched().is_empty()
            || !report.unslotted_entries.is_empty()
            || !report.orphans.is_empty();
        if noteworthy {
            let _ = writeln!(out, "  {name}: {created} created, {updated} updated, {drift} drift");
        }
        for d in report.drifted() {
            let _ = writeln!(out, "    drift: `{d}` read-only region was hand-edited and restored");
        }
        for u in report.unmatched() {
            let _ = writeln!(out, "    error: slot `{u}` names no entry in the bound api-kinograph");
        }
        for o in &report.orphans {
            let _ = writeln!(out, "    warning: read-only block `{o}` has no owning slot (stale; left in place)");
        }
        for e in &report.unslotted_entries {
            let _ = writeln!(out, "    warning: entry `{e}` has no slot in any scanned file");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use kinora::init::init;
    use kinora::kino::{store_kino, StoreKinoParams};
    use stencil::kinds;
    use stencil::target::RustTarget;
    use tempfile::TempDir;

    const SPEC_MD: &str =
        "Creates a user. Errors if the name is empty.\n\n```rust\npub fn new(name: &str) -> Result<User, UserError>;\n```\n";

    fn params(kind: &str, content: &[u8], name: &str) -> StoreKinoParams {
        StoreKinoParams {
            kind: kind.into(),
            content: content.to_vec(),
            author: "t".into(),
            provenance: "t".into(),
            ts: "2026-06-10T10:00:00Z".into(),
            metadata: BTreeMap::from([("name".into(), name.into())]),
            id: None,
            parents: vec![],
        }
    }

    fn store_spec(repo_root: &Path, name: &str, md: &str) -> kinora::event::Event {
        store_kino(&kinora_root(repo_root), params(kinds::API_SPEC, md.as_bytes(), name))
            .unwrap()
            .event
    }

    fn store_kinograph(
        repo_root: &Path,
        name: &str,
        entries: Vec<kinora::kinograph::Entry>,
    ) -> kinora::event::Event {
        let kg = kinora::kinograph::Kinograph { entries };
        let content = kg.to_styxl().unwrap();
        store_kino(&kinora_root(repo_root), params(kinds::API_KINOGRAPH, content.as_bytes(), name))
            .unwrap()
            .event
    }

    /// Store a user-new spec + a `user-api` kinograph referencing it.
    fn seed_user_api(repo_root: &Path) -> kinora::event::Event {
        let spec = store_spec(repo_root, "user-new", SPEC_MD);
        store_kinograph(
            repo_root,
            "user-api",
            vec![kinora::kinograph::Entry::with_id(spec.id.clone())],
        );
        spec
    }

    #[test]
    fn fills_a_slot_and_writes_the_file() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init(root, "https://example.com/x.git").unwrap();
        let spec = seed_user_api(root);

        let file = root.join("user.rs");
        fs::write(&file, "// stencil:kinograph user-api\n// stencil:slot user-new\n").unwrap();

        let summary =
            run_sync(root, root, &["user.rs".into()], &RustTarget).unwrap();

        assert_eq!(summary.files_changed(), 1);
        assert!(!summary.has_errors());
        let written = fs::read_to_string(&file).unwrap();
        assert!(written.contains("/// Creates a user. Errors if the name is empty."));
        assert!(written.contains("pub fn new(name: &str) -> Result<User, UserError>;"));
        assert!(written.contains(&format!("// stencil:ro user-new {}", spec.hash)));
    }

    #[test]
    fn second_run_changes_nothing() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init(root, "https://example.com/x.git").unwrap();
        seed_user_api(root);

        let file = root.join("user.rs");
        fs::write(&file, "// stencil:kinograph user-api\n// stencil:slot user-new\n").unwrap();

        run_sync(root, root, &["user.rs".into()], &RustTarget).unwrap();
        let after_first = fs::read_to_string(&file).unwrap();
        let summary = run_sync(root, root, &["user.rs".into()], &RustTarget).unwrap();

        assert_eq!(summary.files_changed(), 0);
        assert!(!summary.has_errors());
        assert_eq!(fs::read_to_string(&file).unwrap(), after_first);
    }

    #[test]
    fn hand_edited_read_only_region_is_drift_overwritten() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init(root, "https://example.com/x.git").unwrap();
        seed_user_api(root);

        let file = root.join("user.rs");
        fs::write(&file, "// stencil:kinograph user-api\n// stencil:slot user-new\n").unwrap();
        run_sync(root, root, &["user.rs".into()], &RustTarget).unwrap();

        // Tamper inside the read-only region (source hash unchanged).
        let synced = fs::read_to_string(&file).unwrap();
        let tampered = synced.replace(
            "pub fn new(name: &str) -> Result<User, UserError>;",
            "pub fn new() -> User; // sneaky",
        );
        assert_ne!(tampered, synced);
        fs::write(&file, &tampered).unwrap();

        let summary = run_sync(root, root, &["user.rs".into()], &RustTarget).unwrap();
        assert_eq!(summary.files_changed(), 1);
        let report = summary.files[0].result.as_ref().unwrap();
        assert_eq!(report.drifted(), vec!["user-new"]);
        let restored = fs::read_to_string(&file).unwrap();
        assert!(restored.contains("pub fn new(name: &str) -> Result<User, UserError>;"));
        assert!(!restored.contains("sneaky"));
    }

    #[test]
    fn unknown_slot_is_an_error_but_other_files_still_sync() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init(root, "https://example.com/x.git").unwrap();
        seed_user_api(root);

        let good = root.join("good.rs");
        fs::write(&good, "// stencil:kinograph user-api\n// stencil:slot user-new\n").unwrap();
        let bad = root.join("bad.rs");
        fs::write(&bad, "// stencil:kinograph user-api\n// stencil:slot nope\n").unwrap();

        let summary = run_sync(root, root, &[".".into()], &RustTarget).unwrap();
        assert!(summary.has_errors(), "unknown slot must force a non-zero exit");
        // The healthy file is still rendered.
        assert!(fs::read_to_string(&good).unwrap().contains("pub fn new(name: &str)"));
    }

    #[test]
    fn parse_failure_is_reported_per_file() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init(root, "https://example.com/x.git").unwrap();
        seed_user_api(root);

        // A read-only block with no closing `stencil:end` → ParseError.
        let file = root.join("broken.rs");
        fs::write(
            &file,
            "// stencil:kinograph user-api\n// stencil:ro user-new abc\npub fn x();\n",
        )
        .unwrap();

        let summary = run_sync(root, root, &["broken.rs".into()], &RustTarget).unwrap();
        assert!(summary.has_errors());
        assert!(matches!(
            summary.files[0].result,
            Err(StencilError::Parse(_))
        ));
        assert_eq!(summary.files_changed(), 0);
    }

    #[test]
    fn directory_walk_is_recursive_and_skips_target_and_dot_dirs() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init(root, "https://example.com/x.git").unwrap();
        seed_user_api(root);

        let nested = root.join("src").join("inner");
        fs::create_dir_all(&nested).unwrap();
        let deep = nested.join("user.rs");
        fs::write(&deep, "// stencil:kinograph user-api\n// stencil:slot user-new\n").unwrap();

        // Decoys that must be skipped.
        fs::create_dir_all(root.join("target")).unwrap();
        fs::write(
            root.join("target").join("user.rs"),
            "// stencil:kinograph user-api\n// stencil:slot user-new\n",
        )
        .unwrap();
        fs::write(root.join("src").join("notes.txt"), "not rust\n").unwrap();

        let summary = run_sync(root, root, &["src".into()], &RustTarget).unwrap();
        assert_eq!(summary.files_changed(), 1);
        assert!(fs::read_to_string(&deep).unwrap().contains("pub fn new(name: &str)"));
        // target/ file was never touched.
        assert!(!fs::read_to_string(root.join("target").join("user.rs"))
            .unwrap()
            .contains("pub fn new"));
    }

    #[test]
    fn unslotted_entries_are_reported() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init(root, "https://example.com/x.git").unwrap();
        let a = store_spec(root, "user-new", SPEC_MD);
        let b = store_spec(
            root,
            "user-find",
            "Finds a user.\n\n```rust\npub fn find(id: u64) -> Option<User>;\n```\n",
        );
        store_kinograph(
            root,
            "user-api",
            vec![
                kinora::kinograph::Entry::with_id(a.id),
                kinora::kinograph::Entry::with_id(b.id),
            ],
        );

        let file = root.join("user.rs");
        fs::write(&file, "// stencil:kinograph user-api\n// stencil:slot user-new\n").unwrap();

        let summary = run_sync(root, root, &["user.rs".into()], &RustTarget).unwrap();
        let report = summary.files[0].result.as_ref().unwrap();
        assert_eq!(report.unslotted_entries, vec!["user-find"]);
        // Unslotted entries are a warning, not an error.
        assert!(!summary.has_errors());
    }

    #[test]
    fn missing_path_is_a_fatal_error() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init(root, "https://example.com/x.git").unwrap();

        let err = run_sync(root, root, &["does-not-exist.rs".into()], &RustTarget).unwrap_err();
        assert!(matches!(err, CliError::PathNotFound { .. }), "got: {err:?}");
    }

    #[test]
    fn empty_paths_defaults_to_cwd() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init(root, "https://example.com/x.git").unwrap();
        seed_user_api(root);
        let file = root.join("user.rs");
        fs::write(&file, "// stencil:kinograph user-api\n// stencil:slot user-new\n").unwrap();

        // No path arguments → scan cwd (here, the repo root).
        let summary = run_sync(root, root, &[], &RustTarget).unwrap();
        assert_eq!(summary.files_changed(), 1);
        assert!(fs::read_to_string(&file).unwrap().contains("pub fn new(name: &str)"));
    }

    #[test]
    fn absolute_path_argument_is_accepted() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init(root, "https://example.com/x.git").unwrap();
        seed_user_api(root);
        let file = root.join("user.rs");
        fs::write(&file, "// stencil:kinograph user-api\n// stencil:slot user-new\n").unwrap();

        // An absolute arg is used verbatim (cwd here is unrelated `/`).
        let summary =
            run_sync(root, Path::new("/"), &[file.to_string_lossy().into_owned()], &RustTarget)
                .unwrap();
        assert_eq!(summary.files_changed(), 1);
    }

    #[test]
    fn duplicate_spellings_of_one_file_sync_once() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init(root, "https://example.com/x.git").unwrap();
        seed_user_api(root);
        fs::create_dir_all(root.join("sub")).unwrap();
        let file = root.join("sub").join("user.rs");
        fs::write(&file, "// stencil:kinograph user-api\n// stencil:slot user-new\n").unwrap();

        // `sub/user.rs` and `./sub/user.rs` are the same file; the directory
        // `sub` also reaches it. All must collapse to a single scanned file.
        let summary = run_sync(
            root,
            root,
            &["sub/user.rs".into(), "./sub/user.rs".into(), "sub".into()],
            &RustTarget,
        )
        .unwrap();
        assert_eq!(summary.files.len(), 1);
    }

    #[test]
    fn summary_renders_header_and_per_file_tally() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init(root, "https://example.com/x.git").unwrap();
        seed_user_api(root);
        let file = root.join("user.rs");
        fs::write(&file, "// stencil:kinograph user-api\n// stencil:slot user-new\n").unwrap();

        let summary = run_sync(root, root, &["user.rs".into()], &RustTarget).unwrap();
        let text = format_sync_summary(&summary);
        assert!(text.contains("scanned 1 file(s), 1 changed"));
        assert!(text.contains("user.rs: 1 created, 0 updated, 0 drift"));
    }

    #[test]
    fn summary_renders_drift_line() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init(root, "https://example.com/x.git").unwrap();
        seed_user_api(root);
        let file = root.join("user.rs");
        fs::write(&file, "// stencil:kinograph user-api\n// stencil:slot user-new\n").unwrap();
        run_sync(root, root, &["user.rs".into()], &RustTarget).unwrap();

        let tampered = fs::read_to_string(&file).unwrap().replace(
            "pub fn new(name: &str) -> Result<User, UserError>;",
            "pub fn new() -> User; // sneaky",
        );
        fs::write(&file, &tampered).unwrap();
        let summary = run_sync(root, root, &["user.rs".into()], &RustTarget).unwrap();
        let text = format_sync_summary(&summary);
        assert!(text.contains("drift: `user-new`"), "got:\n{text}");
    }

    #[test]
    fn summary_renders_unmatched_error_and_unslotted_warning() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init(root, "https://example.com/x.git").unwrap();
        let a = store_spec(root, "user-new", SPEC_MD);
        let b = store_spec(
            root,
            "user-find",
            "Finds a user.\n\n```rust\npub fn find(id: u64) -> Option<User>;\n```\n",
        );
        store_kinograph(
            root,
            "user-api",
            vec![
                kinora::kinograph::Entry::with_id(a.id),
                kinora::kinograph::Entry::with_id(b.id),
            ],
        );

        // Slot `user-new` matches; slot `nope` does not; `user-find` is unslotted.
        let file = root.join("user.rs");
        fs::write(
            &file,
            "// stencil:kinograph user-api\n// stencil:slot user-new\n// stencil:slot nope\n",
        )
        .unwrap();

        let summary = run_sync(root, root, &["user.rs".into()], &RustTarget).unwrap();
        let text = format_sync_summary(&summary);
        assert!(text.contains("error: slot `nope`"), "got:\n{text}");
        assert!(text.contains("warning: entry `user-find`"), "got:\n{text}");
    }

    #[test]
    fn summary_renders_per_file_error_line() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init(root, "https://example.com/x.git").unwrap();
        seed_user_api(root);
        let file = root.join("broken.rs");
        fs::write(
            &file,
            "// stencil:kinograph user-api\n// stencil:ro user-new abc\npub fn x();\n",
        )
        .unwrap();

        let summary = run_sync(root, root, &["broken.rs".into()], &RustTarget).unwrap();
        let text = format_sync_summary(&summary);
        assert!(text.contains("broken.rs: error:"), "got:\n{text}");
    }

    #[test]
    fn summary_renders_orphan_warning() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init(root, "https://example.com/x.git").unwrap();
        seed_user_api(root);

        // A read-only block whose owning slot was removed, plus a live slot so
        // the file is processed.
        let file = root.join("user.rs");
        fs::write(
            &file,
            concat!(
                "// stencil:kinograph user-api\n",
                "// stencil:ro orphaned abc\n",
                "pub fn gone();\n",
                "// stencil:end\n",
                "// stencil:slot user-new\n",
            ),
        )
        .unwrap();

        let summary = run_sync(root, root, &["user.rs".into()], &RustTarget).unwrap();
        let text = format_sync_summary(&summary);
        assert!(text.contains("warning: read-only block `orphaned`"), "got:\n{text}");
    }
}
