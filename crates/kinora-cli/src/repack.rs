use std::path::Path;

use kinora::author::resolve_author_from_git;
use kinora::repack::{repack_repo, RepackParams, RepackReport};

use crate::common::{find_repo_root, CliError};

pub const DEFAULT_PROVENANCE: &str = "repack";

pub struct RepackRunArgs {
    pub author: Option<String>,
    pub provenance: Option<String>,
}

#[derive(Debug)]
pub struct RepackRunReport {
    pub inner: RepackReport,
}

pub fn run_repack(cwd: &Path, args: RepackRunArgs) -> Result<RepackRunReport, CliError> {
    let repo_root = find_repo_root(cwd)?;

    let author = match args.author {
        Some(a) => a,
        None => resolve_author_from_git(&repo_root).ok_or(CliError::AuthorUnresolved)?,
    };
    let provenance = args.provenance.unwrap_or_else(|| DEFAULT_PROVENANCE.to_owned());
    let ts = jiff::Timestamp::now().to_string();

    let params = RepackParams { author, provenance, ts };
    let inner = repack_repo(&repo_root, params)?;
    Ok(RepackRunReport { inner })
}

/// One-screen human summary printed after `kinora repack` succeeds.
///
/// Pass-through of the clone-stage counters (rebuilt, dropped, rewritten)
/// plus a per-root line for each commit that produced a new version.
/// Roots that committed a no-op don't get a line so the output stays
/// focused on what actually changed.
pub fn format_repack_summary(r: &RepackRunReport) -> String {
    let inner = &r.inner;
    let mut out = String::new();
    out.push_str("repack complete\n");
    for c in &inner.commits {
        if c.new_version.is_some() {
            out.push_str(&format!("  committed root `{}`\n", c.root_name));
        }
    }
    let clone = &inner.clone;
    out.push_str(&format!(
        "{} kino{} rebuilt\n",
        clone.kinos_rebuilt,
        plural_s(clone.kinos_rebuilt),
    ));
    out.push_str(&format!(
        "{} blob{} dropped (unreachable)\n",
        clone.blobs_dropped,
        plural_s(clone.blobs_dropped),
    ));
    out.push_str(&format!(
        "{} filename{} rewritten\n",
        clone.filenames_rewritten,
        plural_s(clone.filenames_rewritten),
    ));
    out.push_str(&format!(
        "{} orphan staged event{} drained",
        inner.orphan_events_drained,
        plural_s(inner.orphan_events_drained),
    ));
    out
}

fn plural_s(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kinora::clone::CloneReport;
    use kinora::init::init;
    use kinora::repack::{RepackCommitEntry, RepackReport};
    use tempfile::TempDir;

    fn repo() -> TempDir {
        let tmp = TempDir::new().unwrap();
        init(tmp.path(), "https://example.com/x.git").unwrap();
        tmp
    }

    fn args() -> RepackRunArgs {
        RepackRunArgs {
            author: Some("yj".into()),
            provenance: Some("cli-test".into()),
        }
    }

    #[test]
    fn run_repack_succeeds_on_empty_repo() {
        let tmp = repo();
        let r = run_repack(tmp.path(), args()).unwrap();
        assert_eq!(r.inner.clone.kinos_rebuilt, 0);
        assert_eq!(r.inner.clone.blobs_dropped, 0);
        assert_eq!(r.inner.clone.filenames_rewritten, 0);
    }

    #[test]
    fn run_repack_errors_outside_kinora_repo() {
        let tmp = TempDir::new().unwrap();
        let err = run_repack(tmp.path(), args()).unwrap_err();
        assert!(matches!(err, CliError::NotInKinoraRepo { .. }));
    }

    #[test]
    fn run_repack_errors_when_author_unresolved() {
        let tmp = repo();
        let mut a = args();
        a.author = None;
        let err = run_repack(tmp.path(), a).unwrap_err();
        assert!(matches!(err, CliError::AuthorUnresolved));
    }

    #[test]
    fn run_repack_provenance_defaults_to_repack_when_omitted() {
        let tmp = repo();
        let mut a = args();
        a.provenance = None;
        run_repack(tmp.path(), a).unwrap();
    }

    #[test]
    fn format_summary_zero_counts_has_no_commit_lines() {
        let r = RepackRunReport { inner: RepackReport::default() };
        let s = format_repack_summary(&r);
        assert!(s.contains("repack complete"), "got: {s}");
        assert!(s.contains("0 kinos rebuilt"), "got: {s}");
        assert!(s.contains("0 blobs dropped"), "got: {s}");
        assert!(s.contains("0 filenames rewritten"), "got: {s}");
        assert!(s.contains("0 orphan staged events drained"), "got: {s}");
        assert!(!s.contains("committed root"), "no commits means no commit lines: {s}");
    }

    #[test]
    fn format_summary_singular_forms_for_one() {
        let r = RepackRunReport {
            inner: RepackReport {
                commits: vec![RepackCommitEntry {
                    root_name: "main".into(),
                    new_version: Some("a".repeat(64)),
                    prior_version: None,
                }],
                clone: CloneReport {
                    kinos_rebuilt: 1,
                    blobs_dropped: 1,
                    filenames_rewritten: 1,
                },
                orphan_events_drained: 1,
            },
        };
        let s = format_repack_summary(&r);
        assert!(s.contains("committed root `main`"), "got: {s}");
        assert!(s.contains("1 kino rebuilt"), "got: {s}");
        assert!(s.contains("1 blob dropped"), "got: {s}");
        assert!(s.contains("1 filename rewritten"), "got: {s}");
        assert!(s.contains("1 orphan staged event drained"), "got: {s}");
    }

    #[test]
    fn format_summary_skips_noop_commit_lines() {
        let r = RepackRunReport {
            inner: RepackReport {
                commits: vec![
                    RepackCommitEntry {
                        root_name: "quiet".into(),
                        new_version: None,
                        prior_version: Some("p".repeat(64)),
                    },
                    RepackCommitEntry {
                        root_name: "loud".into(),
                        new_version: Some("a".repeat(64)),
                        prior_version: None,
                    },
                ],
                clone: CloneReport::default(),
                orphan_events_drained: 0,
            },
        };
        let s = format_repack_summary(&r);
        assert!(s.contains("committed root `loud`"), "loud committed: {s}");
        assert!(!s.contains("committed root `quiet`"), "quiet is no-op: {s}");
    }
}
