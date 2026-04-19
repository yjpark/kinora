use std::path::Path;

use kinora::author::resolve_author_from_git;
use kinora::compact::{compact_all, CompactAllEntry, CompactParams};
use kinora::paths::kinora_root;

use crate::common::{find_repo_root, CliError};

pub const DEFAULT_PROVENANCE: &str = "compact";

pub struct CompactRunArgs {
    pub author: Option<String>,
    pub provenance: Option<String>,
}

/// Outcome of `kinora compact`: one entry per declared root in name
/// order. The outer `Result` on `run_compact` is reserved for failures
/// before any root was visited (config load, author resolution); once
/// iteration starts, per-root errors land in the entry itself so clean
/// roots still advance to disk.
#[derive(Debug)]
pub struct CompactRunReport {
    pub per_root: Vec<CompactAllEntry>,
}

impl CompactRunReport {
    /// True iff at least one root's compaction returned an `Err`.
    pub fn any_error(&self) -> bool {
        self.per_root.iter().any(|(_, r)| r.is_err())
    }
}

pub fn run_compact(cwd: &Path, args: CompactRunArgs) -> Result<CompactRunReport, CliError> {
    let repo_root = find_repo_root(cwd)?;
    let kin_root = kinora_root(&repo_root);

    let author = match args.author {
        Some(a) => a,
        None => resolve_author_from_git(&repo_root).ok_or(CliError::AuthorUnresolved)?,
    };
    let provenance = args.provenance.unwrap_or_else(|| DEFAULT_PROVENANCE.to_owned());
    let ts = jiff::Timestamp::now().to_string();

    let params = CompactParams { author, provenance, ts };
    let per_root = compact_all(&kin_root, params)?;
    Ok(CompactRunReport { per_root })
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

    fn args() -> CompactRunArgs {
        CompactRunArgs {
            author: Some("YJ".into()),
            provenance: Some("cli-test".into()),
        }
    }

    fn store_md(root: &std::path::Path, content: &[u8], name: &str) {
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
    fn run_compact_without_root_flag_compacts_every_declared_root() {
        let (tmp, kin) = repo();
        write_multi_root_config(&kin, &["main", "rfcs"]);
        store_md(&kin, b"x", "x");

        let report = run_compact(tmp.path(), args()).unwrap();
        // inbox is auto-provisioned in Config::from_styx when absent.
        let names: Vec<_> = report.per_root.iter().map(|(n, _)| n.clone()).collect();
        assert_eq!(names, vec!["inbox", "main", "rfcs"]);
        assert!(!report.any_error(), "expected all roots to succeed: {names:?}");
        assert!(kinora::paths::root_pointer_path(&kin, "main").is_file());
        assert!(kinora::paths::root_pointer_path(&kin, "rfcs").is_file());
    }

    #[test]
    fn run_compact_any_error_flag_flips_when_one_root_fails() {
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
        let report = run_compact(tmp.path(), args()).unwrap();
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
    fn run_compact_no_op_line_when_nothing_to_promote() {
        let (tmp, _kin) = repo();
        // Default config has only `inbox`, and no hot events → no-op.
        let report = run_compact(tmp.path(), args()).unwrap();
        assert!(!report.any_error());
        assert_eq!(report.per_root.len(), 1);
        let (name, result) = &report.per_root[0];
        assert_eq!(name, "inbox");
        let r = result.as_ref().unwrap();
        assert!(r.new_version.is_none(), "no hot events → no new version");
    }

    #[test]
    fn run_compact_errors_outside_kinora_repo() {
        let tmp = TempDir::new().unwrap();
        let err = run_compact(tmp.path(), args()).unwrap_err();
        assert!(matches!(err, CliError::NotInKinoraRepo { .. }));
    }

    #[test]
    fn run_compact_errors_when_author_unresolved() {
        let (tmp, _kin) = repo();
        let mut a = args();
        a.author = None;
        let err = run_compact(tmp.path(), a).unwrap_err();
        assert!(matches!(err, CliError::AuthorUnresolved));
    }

    #[test]
    fn compact_subcommand_rejects_removed_root_flag() {
        // Per D5 / hxmw-l79b, `--root` on `kinora compact` was retired.
        // figue should reject the flag as unknown.
        let outcome = figue::from_slice::<Cli>(&["compact", "--root", "main"]).into_result();
        assert!(
            outcome.is_err(),
            "figue should reject --root on compact; got Ok(_)"
        );
    }
}
