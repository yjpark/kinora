use std::path::Path;

use kinora::author::resolve_author_from_git;
use kinora::paths::kinora_root;
use kinora::reformat::{reformat_repo, ReformatParams, ReformatReport};

use crate::common::{find_repo_root, CliError};

pub const DEFAULT_PROVENANCE: &str = "reformat";

pub struct ReformatRunArgs {
    pub author: Option<String>,
    pub provenance: Option<String>,
}

#[derive(Debug)]
pub struct ReformatRunReport {
    pub inner: ReformatReport,
}

pub fn run_reformat(cwd: &Path, args: ReformatRunArgs) -> Result<ReformatRunReport, CliError> {
    let repo_root = find_repo_root(cwd)?;
    let kin_root = kinora_root(&repo_root);

    let author = match args.author {
        Some(a) => a,
        None => resolve_author_from_git(&repo_root).ok_or(CliError::AuthorUnresolved)?,
    };
    let provenance = args.provenance.unwrap_or_else(|| DEFAULT_PROVENANCE.to_owned());
    let ts = jiff::Timestamp::now().to_string();

    let params = ReformatParams { author, provenance, ts };
    let inner = reformat_repo(&kin_root, params)?;
    Ok(ReformatRunReport { inner })
}

/// One-screen human summary printed after `kinora reformat` succeeds.
///
/// Two counters per kind (kinographs, roots): how many were reformatted vs
/// already in the target format. If any kinographs were staged, the summary
/// ends with a line suggesting `kinora commit` so the user knows that
/// staging is only the first step.
pub fn format_reformat_summary(r: &ReformatRunReport) -> String {
    let inner = &r.inner;
    let kg_new = inner.reformatted_kinographs.len();
    let kg_skipped = inner.skipped_kinographs_already_formatted;
    let root_new = inner.reformatted_roots.len();
    let root_skipped = inner.skipped_roots_already_formatted;

    let mut out = String::new();
    out.push_str(&format!(
        "reformatted {} kinograph{} ({} already styxl)\n",
        kg_new,
        plural_s(kg_new),
        kg_skipped,
    ));
    out.push_str(&format!(
        "reformatted {} root{} ({} already styxl)",
        root_new,
        plural_s(root_new),
        root_skipped,
    ));
    if kg_new > 0 {
        out.push_str("\nrun `kinora commit` to promote the staged versions to heads");
    }
    out
}

fn plural_s(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kinora::init::init;
    use kinora::reformat::{ReformattedKinograph, ReformattedRoot};
    use tempfile::TempDir;

    fn repo() -> TempDir {
        let tmp = TempDir::new().unwrap();
        init(tmp.path(), "https://example.com/x.git").unwrap();
        tmp
    }

    fn args() -> ReformatRunArgs {
        ReformatRunArgs {
            author: Some("YJ".into()),
            provenance: Some("cli-test".into()),
        }
    }

    #[test]
    fn run_reformat_succeeds_on_empty_repo() {
        let tmp = repo();
        let r = run_reformat(tmp.path(), args()).unwrap();
        assert!(r.inner.reformatted_kinographs.is_empty());
        assert!(r.inner.reformatted_roots.is_empty());
        assert_eq!(r.inner.skipped_kinographs_already_formatted, 0);
        assert_eq!(r.inner.skipped_roots_already_formatted, 0);
    }

    #[test]
    fn run_reformat_errors_outside_kinora_repo() {
        let tmp = TempDir::new().unwrap();
        let err = run_reformat(tmp.path(), args()).unwrap_err();
        assert!(matches!(err, CliError::NotInKinoraRepo { .. }));
    }

    #[test]
    fn run_reformat_errors_when_author_unresolved() {
        let tmp = repo();
        let mut a = args();
        a.author = None;
        let err = run_reformat(tmp.path(), a).unwrap_err();
        assert!(matches!(err, CliError::AuthorUnresolved));
    }

    #[test]
    fn run_reformat_provenance_defaults_to_reformat_when_omitted() {
        let tmp = repo();
        let mut a = args();
        a.provenance = None;
        // Repo is empty so nothing is reformatted; we just need the run to
        // succeed without panicking on the default provenance path.
        let r = run_reformat(tmp.path(), a).unwrap();
        assert!(r.inner.reformatted_kinographs.is_empty());
    }

    #[test]
    fn format_summary_zero_counts_no_commit_hint() {
        let r = ReformatRunReport { inner: ReformatReport::default() };
        let s = format_reformat_summary(&r);
        assert!(
            s.contains("reformatted 0 kinographs (0 already styxl)"),
            "got: {s}"
        );
        assert!(
            s.contains("reformatted 0 roots (0 already styxl)"),
            "got: {s}"
        );
        assert!(
            !s.contains("kinora commit"),
            "no commit hint when nothing staged: {s}"
        );
    }

    #[test]
    fn format_summary_singular_forms_for_one() {
        let inner = ReformatReport {
            reformatted_kinographs: vec![ReformattedKinograph {
                id: "a".repeat(64),
                prior_version: "b".repeat(64),
                new_version: "c".repeat(64),
            }],
            skipped_kinographs_already_formatted: 0,
            reformatted_roots: vec![ReformattedRoot {
                root_name: "main".into(),
                prior_version: "d".repeat(64),
                new_version: "e".repeat(64),
            }],
            skipped_roots_already_formatted: 0,
        };
        let r = ReformatRunReport { inner };
        let s = format_reformat_summary(&r);
        assert!(s.contains("reformatted 1 kinograph (0 already styxl)"), "got: {s}");
        assert!(s.contains("reformatted 1 root (0 already styxl)"), "got: {s}");
    }

    #[test]
    fn format_summary_plural_forms_for_multiple() {
        let inner = ReformatReport {
            reformatted_kinographs: vec![],
            skipped_kinographs_already_formatted: 3,
            reformatted_roots: vec![],
            skipped_roots_already_formatted: 2,
        };
        let r = ReformatRunReport { inner };
        let s = format_reformat_summary(&r);
        assert!(s.contains("reformatted 0 kinographs (3 already styxl)"), "got: {s}");
        assert!(s.contains("reformatted 0 roots (2 already styxl)"), "got: {s}");
    }

    #[test]
    fn format_summary_suggests_commit_when_kinographs_staged() {
        let inner = ReformatReport {
            reformatted_kinographs: vec![ReformattedKinograph {
                id: "a".repeat(64),
                prior_version: "b".repeat(64),
                new_version: "c".repeat(64),
            }],
            skipped_kinographs_already_formatted: 0,
            reformatted_roots: vec![],
            skipped_roots_already_formatted: 0,
        };
        let r = ReformatRunReport { inner };
        let s = format_reformat_summary(&r);
        assert!(s.contains("kinora commit"), "expected commit hint: {s}");
    }

    #[test]
    fn format_summary_no_commit_hint_when_only_roots_reformatted() {
        let inner = ReformatReport {
            reformatted_kinographs: vec![],
            skipped_kinographs_already_formatted: 0,
            reformatted_roots: vec![ReformattedRoot {
                root_name: "main".into(),
                prior_version: "d".repeat(64),
                new_version: "e".repeat(64),
            }],
            skipped_roots_already_formatted: 0,
        };
        let r = ReformatRunReport { inner };
        let s = format_reformat_summary(&r);
        // Roots are rewritten in place — no staged commit needed to surface them.
        assert!(
            !s.contains("kinora commit"),
            "commit hint should only appear when kinographs were staged: {s}"
        );
    }
}
