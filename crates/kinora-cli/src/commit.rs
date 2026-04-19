use std::io::{self, Write};
use std::path::Path;

use kinora::author::resolve_author_from_git;
use kinora::commit::{commit_all, CommitAllEntry, CommitError, CommitParams};
use kinora::paths::kinora_root;

use crate::common::{find_repo_root, CliError};

pub const DEFAULT_PROVENANCE: &str = "commit";

const SHORTHASH_LEN: usize = 8;

fn short(hex: &str) -> &str {
    &hex[..hex.len().min(SHORTHASH_LEN)]
}

pub struct CommitRunArgs {
    pub author: Option<String>,
    pub provenance: Option<String>,
}

/// Outcome of `kinora commit`: one entry per declared root in name
/// order. The outer `Result` on `run_commit` is reserved for failures
/// before any root was visited (config load, author resolution); once
/// iteration starts, per-root errors land in the entry itself so clean
/// roots still advance to disk.
#[derive(Debug)]
pub struct CommitRunReport {
    pub per_root: Vec<CommitAllEntry>,
}

impl CommitRunReport {
    /// True iff at least one root's commit returned an `Err`.
    pub fn any_error(&self) -> bool {
        self.per_root.iter().any(|(_, r)| r.is_err())
    }
}

pub fn run_commit(cwd: &Path, args: CommitRunArgs) -> Result<CommitRunReport, CliError> {
    let repo_root = find_repo_root(cwd)?;
    let kin_root = kinora_root(&repo_root);

    let author = match args.author {
        Some(a) => a,
        None => resolve_author_from_git(&repo_root).ok_or(CliError::AuthorUnresolved)?,
    };
    let provenance = args.provenance.unwrap_or_else(|| DEFAULT_PROVENANCE.to_owned());
    let ts = jiff::Timestamp::now().to_string();

    let params = CommitParams { author, provenance, ts };
    let per_root = commit_all(&kin_root, params)?;
    Ok(CommitRunReport { per_root })
}

/// Render a single per-root commit entry. Success cases render as a short
/// `version=` line; failures fall through to `render_commit_error`.
pub fn render_commit_entry<W: Write>(w: &mut W, entry: &CommitAllEntry) -> io::Result<()> {
    let (name, result) = entry;
    match result {
        Ok(r) => {
            let retention_hint = render_retention_hint(&r.retained_by_cross_root);
            match &r.new_version {
                Some(h) => writeln!(
                    w,
                    "root={} version={} (new version{retention_hint})",
                    name,
                    h.shorthash()
                ),
                None => {
                    let version = r
                        .prior_version
                        .as_ref()
                        .map(|h| h.shorthash().to_owned())
                        .unwrap_or_else(|| "-".into());
                    writeln!(w, "root={name} version={version} (no-op{retention_hint})")
                }
            }
        }
        Err(e) => render_commit_error(w, name, e),
    }
}

/// Render the cross-root retention hint as a trailing clause inside the
/// status parens. Returns an empty string when no entries were rescued
/// by a cross-root reference. Format:
///
/// ```text
/// ; 2 entries retained by cross-root refs from main
/// ; 3 entries retained by cross-root refs from main, rfcs
/// ```
///
/// Root names are listed in the BTreeMap's natural sort order. Counts
/// sum across all referencing roots — a single entry referenced by two
/// roots contributes to both tallies, so the leading number is the
/// total-retention count, not the unique-entry count.
fn render_retention_hint(retained: &std::collections::BTreeMap<String, usize>) -> String {
    if retained.is_empty() {
        return String::new();
    }
    let total: usize = retained.values().sum();
    let roots: Vec<&str> = retained.keys().map(|s| s.as_str()).collect();
    let plural = if total == 1 { "entry" } else { "entries" };
    format!(
        "; {total} {plural} retained by cross-root refs from {}",
        roots.join(", ")
    )
}

/// Render a `CommitError` under a named root. `AmbiguousAssign` and
/// `UnknownRoot` get the D2 multi-line format so the user sees the
/// candidates and a copy-pasteable resolution hint; other variants fall
/// back to a single `root=X ERROR: <display>` line.
///
/// Note the intentional double-space after `root=<name>` for the two
/// structured variants — matches the D2 mock-up in bean 7mou and flags
/// these (fixable) config/user errors against the generic fallback.
pub fn render_commit_error<W: Write>(
    w: &mut W,
    root_name: &str,
    err: &CommitError,
) -> io::Result<()> {
    match err {
        CommitError::AmbiguousAssign { kino_id, candidates } => {
            writeln!(
                w,
                "root={root_name}  ERROR: ambiguous assigns for kino {}…",
                short(kino_id)
            )?;
            let target_width = candidates
                .iter()
                .map(|c| c.target_root.len())
                .max()
                .unwrap_or(0);
            for c in candidates {
                writeln!(
                    w,
                    "  - assign → {:<width$} (event {}…, {}, {})",
                    c.target_root,
                    short(&c.event_hash),
                    c.author,
                    c.ts,
                    width = target_width,
                )?;
            }
            let event_list = candidates
                .iter()
                .map(|c| format!("{}…", short(&c.event_hash)))
                .collect::<Vec<_>>()
                .join(",");
            writeln!(
                w,
                "to resolve: kinora assign {}… <root> --resolves {event_list}",
                short(kino_id),
            )?;
        }
        CommitError::UnknownRoot { name, event_hash } => {
            writeln!(
                w,
                "root={root_name}  ERROR: unknown root `{name}` referenced by assign event {}…",
                short(event_hash),
            )?;
        }
        other => {
            writeln!(w, "root={root_name} ERROR: {other}")?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kinora::init::init;
    use kinora::kino::{store_kino, StoreKinoParams};
    use kinora::paths::config_path;
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    use crate::cli::Cli;

    fn repo() -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        init(tmp.path(), "https://example.com/x.git").unwrap();
        let root = kinora_root(tmp.path());
        (tmp, root)
    }

    fn args() -> CommitRunArgs {
        CommitRunArgs {
            author: Some("YJ".into()),
            provenance: Some("cli-test".into()),
        }
    }

    fn store_md(root: &std::path::Path, content: &[u8], name: &str) -> kinora::event::Event {
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
        .unwrap()
        .event
    }

    fn assign_to(root: &std::path::Path, kino_id: &str, target_root: &str) {
        kinora::assign::write_assign(
            root,
            &kinora::assign::AssignEvent {
                kino_id: kino_id.to_owned(),
                target_root: target_root.to_owned(),
                supersedes: vec![],
                author: "yj".into(),
                ts: "2026-04-19T10:00:01Z".into(),
                provenance: "cli-test".into(),
            },
        )
        .unwrap();
    }

    fn write_multi_root_config(kin: &std::path::Path, names: &[&str]) {
        let mut body = String::from("repo-url \"https://example.com/x.git\"\nroots {\n");
        for n in names {
            body.push_str(&format!("  {n} {{ policy \"never\" }}\n"));
        }
        body.push_str("}\n");
        fs::write(config_path(kin), body).unwrap();
    }

    #[test]
    fn run_commit_without_root_flag_commits_every_declared_root() {
        let (tmp, kin) = repo();
        write_multi_root_config(&kin, &["main", "rfcs"]);
        // Assign one kino to main and one to rfcs so both advance to disk.
        let a = store_md(&kin, b"a", "a");
        let b = store_md(&kin, b"b", "b");
        assign_to(&kin, &a.id, "main");
        assign_to(&kin, &b.id, "rfcs");

        let report = run_commit(tmp.path(), args()).unwrap();
        // Non-commits roots run in name order (auto-provisioned `inbox`
        // included), then `commits` iterates last.
        let names: Vec<_> = report.per_root.iter().map(|(n, _)| n.clone()).collect();
        assert_eq!(names, vec!["inbox", "main", "rfcs", "commits"]);
        assert!(!report.any_error(), "expected all roots to succeed: {names:?}");
        assert!(kinora::paths::root_pointer_path(&kin, "main").is_file());
        assert!(kinora::paths::root_pointer_path(&kin, "rfcs").is_file());
    }

    #[test]
    fn run_commit_any_error_flag_flips_when_one_root_fails() {
        let (tmp, kin) = repo();
        write_multi_root_config(&kin, &["main", "broken"]);

        // Sabotage `broken` with a pointer referencing a missing event.
        fs::create_dir_all(kinora::paths::roots_dir(&kin)).unwrap();
        fs::write(
            kinora::paths::root_pointer_path(&kin, "broken"),
            "ff".repeat(32),
        )
        .unwrap();

        store_md(&kin, b"x", "x");
        let report = run_commit(tmp.path(), args()).unwrap();
        assert!(report.any_error(), "broken root should flip any_error");
        let by_name: std::collections::HashMap<_, _> = report
            .per_root
            .iter()
            .map(|(n, r)| (n.clone(), r))
            .collect();
        assert!(by_name["main"].is_ok());
        assert!(by_name["broken"].is_err());
    }

    #[test]
    fn run_commit_no_op_line_when_nothing_to_promote() {
        let (tmp, _kin) = repo();
        // Default config has `inbox` and `commits` (both auto-provisioned),
        // and no staged events → no-op for each.
        let report = run_commit(tmp.path(), args()).unwrap();
        assert!(!report.any_error());
        assert_eq!(report.per_root.len(), 2);
        let names: Vec<_> = report.per_root.iter().map(|(n, _)| n.clone()).collect();
        // `commits` always runs last so it can sweep up archive-assigns.
        assert_eq!(names, vec!["inbox", "commits"]);
        for (_name, result) in &report.per_root {
            let r = result.as_ref().unwrap();
            assert!(r.new_version.is_none(), "no staged events → no new version");
        }
    }

    #[test]
    fn run_commit_errors_outside_kinora_repo() {
        let tmp = TempDir::new().unwrap();
        let err = run_commit(tmp.path(), args()).unwrap_err();
        assert!(matches!(err, CliError::NotInKinoraRepo { .. }));
    }

    #[test]
    fn run_commit_errors_when_author_unresolved() {
        let (tmp, _kin) = repo();
        let mut a = args();
        a.author = None;
        let err = run_commit(tmp.path(), a).unwrap_err();
        assert!(matches!(err, CliError::AuthorUnresolved));
    }

    #[test]
    fn commit_subcommand_rejects_removed_root_flag() {
        // Per D5 / hxmw-l79b, `--root` on `kinora commit` was retired.
        // figue should reject the flag as unknown.
        let outcome = figue::from_slice::<Cli>(&["commit", "--root", "main"]).into_result();
        assert!(
            outcome.is_err(),
            "figue should reject --root on commit; got Ok(_)"
        );
    }

    // ---- D2 CLI rendering for AmbiguousAssign / UnknownRoot ----

    fn render_err(root_name: &str, err: &CommitError) -> String {
        let mut buf: Vec<u8> = Vec::new();
        render_commit_error(&mut buf, root_name, err).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn ambiguous_assign_renders_d2_block_with_resolution_hint() {
        let kino_id = "a".repeat(64);
        let candidates = vec![
            kinora::commit::AssignCandidate {
                event_hash: format!("{}{}", "abc1", "0".repeat(60)),
                target_root: "rfcs".into(),
                author: "yj".into(),
                ts: "2026-04-19T10:00:00Z".into(),
            },
            kinora::commit::AssignCandidate {
                event_hash: format!("{}{}", "def2", "0".repeat(60)),
                target_root: "designs".into(),
                author: "yj".into(),
                ts: "2026-04-19T11:00:00Z".into(),
            },
        ];
        let err = CommitError::AmbiguousAssign { kino_id: kino_id.clone(), candidates };
        let out = render_err("rfcs", &err);
        let expected = "\
root=rfcs  ERROR: ambiguous assigns for kino aaaaaaaa…
  - assign → rfcs    (event abc10000…, yj, 2026-04-19T10:00:00Z)
  - assign → designs (event def20000…, yj, 2026-04-19T11:00:00Z)
to resolve: kinora assign aaaaaaaa… <root> --resolves abc10000…,def20000…
";
        assert_eq!(out, expected);
    }

    #[test]
    fn unknown_root_renders_single_line_with_offending_event() {
        let err = CommitError::UnknownRoot {
            name: "madeup".into(),
            event_hash: format!("{}{}", "xyz12345", "0".repeat(56)),
        };
        let out = render_err("main", &err);
        assert_eq!(
            out,
            "root=main  ERROR: unknown root `madeup` referenced by assign event xyz12345…\n"
        );
    }

    #[test]
    fn other_commit_errors_fall_back_to_single_line_format() {
        let err = CommitError::NoHead { id: "z".repeat(64) };
        let out = render_err("main", &err);
        assert!(
            out.starts_with("root=main ERROR: "),
            "fallback format must be single-line; got: {out:?}"
        );
        assert!(out.ends_with('\n'));
    }

    #[test]
    fn render_commit_entry_ok_with_new_version_uses_shorthash() {
        use std::str::FromStr;
        let hash = kinora::hash::Hash::from_str(&"f".repeat(64)).unwrap();
        let entry: CommitAllEntry = (
            "main".into(),
            Ok(kinora::commit::CommitResult {
                root_name: "main".into(),
                new_version: Some(hash.clone()),
                prior_version: None,
                retained_by_cross_root: std::collections::BTreeMap::new(),
            }),
        );
        let mut buf: Vec<u8> = Vec::new();
        render_commit_entry(&mut buf, &entry).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert_eq!(out, format!("root=main version={} (new version)\n", hash.shorthash()));
    }

    #[test]
    fn render_commit_entry_ok_no_op_with_no_prior_renders_dash() {
        let entry: CommitAllEntry = (
            "inbox".into(),
            Ok(kinora::commit::CommitResult {
                root_name: "inbox".into(),
                new_version: None,
                prior_version: None,
                retained_by_cross_root: std::collections::BTreeMap::new(),
            }),
        );
        let mut buf: Vec<u8> = Vec::new();
        render_commit_entry(&mut buf, &entry).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert_eq!(out, "root=inbox version=- (no-op)\n");
    }

    #[test]
    fn render_commit_entry_appends_retention_hint_from_single_root() {
        use std::str::FromStr;
        let hash = kinora::hash::Hash::from_str(&"a".repeat(64)).unwrap();
        let entry: CommitAllEntry = (
            "inbox".into(),
            Ok(kinora::commit::CommitResult {
                root_name: "inbox".into(),
                new_version: Some(hash.clone()),
                prior_version: None,
                retained_by_cross_root: std::collections::BTreeMap::from([
                    ("main".to_string(), 2),
                ]),
            }),
        );
        let mut buf: Vec<u8> = Vec::new();
        render_commit_entry(&mut buf, &entry).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert_eq!(
            out,
            format!(
                "root=inbox version={} (new version; 2 entries retained by cross-root refs from main)\n",
                hash.shorthash()
            )
        );
    }

    #[test]
    fn render_commit_entry_appends_retention_hint_from_multiple_roots_sorted() {
        use std::str::FromStr;
        let hash = kinora::hash::Hash::from_str(&"b".repeat(64)).unwrap();
        let entry: CommitAllEntry = (
            "rfcs".into(),
            Ok(kinora::commit::CommitResult {
                root_name: "rfcs".into(),
                new_version: Some(hash.clone()),
                prior_version: None,
                retained_by_cross_root: std::collections::BTreeMap::from([
                    ("zeta".to_string(), 1),
                    ("main".to_string(), 2),
                ]),
            }),
        );
        let mut buf: Vec<u8> = Vec::new();
        render_commit_entry(&mut buf, &entry).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert_eq!(
            out,
            format!(
                "root=rfcs version={} (new version; 3 entries retained by cross-root refs from main, zeta)\n",
                hash.shorthash()
            )
        );
    }

    #[test]
    fn render_commit_entry_retention_hint_uses_singular_for_one_entry() {
        use std::str::FromStr;
        let hash = kinora::hash::Hash::from_str(&"c".repeat(64)).unwrap();
        let entry: CommitAllEntry = (
            "inbox".into(),
            Ok(kinora::commit::CommitResult {
                root_name: "inbox".into(),
                new_version: Some(hash.clone()),
                prior_version: None,
                retained_by_cross_root: std::collections::BTreeMap::from([
                    ("main".to_string(), 1),
                ]),
            }),
        );
        let mut buf: Vec<u8> = Vec::new();
        render_commit_entry(&mut buf, &entry).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(
            out.contains("1 entry retained"),
            "singular form for 1 entry: {out:?}"
        );
    }

    #[test]
    fn render_commit_entry_retention_hint_attaches_to_no_op_too() {
        let entry: CommitAllEntry = (
            "inbox".into(),
            Ok(kinora::commit::CommitResult {
                root_name: "inbox".into(),
                new_version: None,
                prior_version: None,
                retained_by_cross_root: std::collections::BTreeMap::from([
                    ("main".to_string(), 1),
                ]),
            }),
        );
        let mut buf: Vec<u8> = Vec::new();
        render_commit_entry(&mut buf, &entry).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert_eq!(
            out,
            "root=inbox version=- (no-op; 1 entry retained by cross-root refs from main)\n"
        );
    }
}
