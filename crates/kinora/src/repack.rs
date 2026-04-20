//! Compose `commit` + `clone` + directory swap into a single atomic
//! operation. Drops unreachable blobs and rewrites legacy filenames into
//! the canonical form without the user having to drive three commands by
//! hand.
//!
//! Flow (all operations relative to the enclosing repo root, not the
//! `.kinora/` dir):
//!
//! 1. **Preflight** — refuse to run if `<repo>/.kinora.repack-tmp` or
//!    `<repo>/.kinora.repack-old` already exist. A lingering temp means a
//!    prior repack crashed and needs manual attention.
//! 2. **Commit** — run [`commit_all`] on `<repo>/.kinora/`. Pending
//!    staged events get promoted so they survive the rebuild. If any root
//!    fails, the whole repack bails before the clone.
//! 3. **Clone** — [`clone_repo`] from `<repo>/.kinora/` into
//!    `<repo>/.kinora.repack-tmp/`. Sibling path so the swap is a rename
//!    within one filesystem.
//! 4. **Swap** — rename `<repo>/.kinora` → `<repo>/.kinora.repack-old`,
//!    then rename `<repo>/.kinora.repack-tmp` → `<repo>/.kinora`. If the
//!    second rename fails, the first is rolled back.
//! 5. **Cleanup** — delete `<repo>/.kinora.repack-old` on success.
//!
//! Repack is hash-preserving: it composes clone, which never rewrites
//! content bytes. Content migrations (e.g. legacy styx → styxl) still
//! go through `kinora::reformat`.

use std::io;
use std::path::{Path, PathBuf};

use crate::clone::{CloneError, CloneReport};
use crate::commit::CommitError;
use crate::paths::KINORA_DIR;

pub const TMP_SUFFIX: &str = ".repack-tmp";
pub const OLD_SUFFIX: &str = ".repack-old";

#[derive(Debug, thiserror::Error)]
pub enum RepackError {
    #[error("repack io error: {0}")]
    Io(#[from] io::Error),
    #[error(transparent)]
    Commit(#[from] CommitError),
    #[error(transparent)]
    Clone(#[from] CloneError),
    #[error("lingering repack temp directory: {}", .path.display())]
    TempExists { path: PathBuf },
    #[error("lingering repack old directory: {}", .path.display())]
    OldExists { path: PathBuf },
    #[error("`{root_name}` commit failed during repack: {err}")]
    CommitRootFailed {
        root_name: String,
        #[source]
        err: Box<CommitError>,
    },
    #[error("swap failed after clone; rolled back. Original `.kinora/` is intact: {0}")]
    SwapFailed(#[source] io::Error),
}

/// Caller-supplied parameters. Passed through to the underlying commit
/// and clone calls.
#[derive(Debug, Clone)]
pub struct RepackParams {
    pub author: String,
    pub provenance: String,
    pub ts: String,
}

/// Per-root commit outcome that survived the repack. Mirrors the subset
/// of `CommitResult` callers typically want for human output.
#[derive(Debug, Clone)]
pub struct RepackCommitEntry {
    pub root_name: String,
    pub new_version: Option<String>,
    pub prior_version: Option<String>,
}

#[derive(Debug, Default, Clone)]
pub struct RepackReport {
    pub commits: Vec<RepackCommitEntry>,
    pub clone: CloneReport,
}

/// Run repack against the repo rooted at `repo_root` (the directory
/// *containing* `.kinora/`, not `.kinora/` itself).
pub fn repack_repo(
    _repo_root: &Path,
    _params: RepackParams,
) -> Result<RepackReport, RepackError> {
    todo!("impl pending")
}

#[allow(dead_code)]
pub(crate) fn sibling(repo_root: &Path, suffix: &str) -> PathBuf {
    repo_root.join(format!("{KINORA_DIR}{suffix}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commit::{commit_all, CommitParams};
    use crate::init::init;
    use crate::kino::{store_kino, StoreKinoParams};
    use crate::paths::kinora_root;
    use std::collections::BTreeMap;
    use std::fs;
    use tempfile::TempDir;

    fn setup() -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        init(tmp.path(), "https://example.com/x.git").unwrap();
        let repo = tmp.path().to_path_buf();
        (tmp, repo)
    }

    fn repack_params(ts: &str) -> RepackParams {
        RepackParams {
            author: "yj".into(),
            provenance: "repack-test".into(),
            ts: ts.into(),
        }
    }

    fn store_params(ts: &str) -> StoreKinoParams {
        StoreKinoParams {
            kind: "markdown".into(),
            content: b"# hello\n".to_vec(),
            id: None,
            parents: vec![],
            metadata: BTreeMap::from([("name".into(), "hello".into())]),
            author: "yj".into(),
            provenance: "repack-test".into(),
            ts: ts.into(),
        }
    }

    #[test]
    fn repack_succeeds_on_empty_repo() {
        let (_tmp, repo) = setup();
        let r = repack_repo(&repo, repack_params("2026-04-20T00:00:00Z")).unwrap();
        assert_eq!(r.clone.kinos_rebuilt, 0);
        assert_eq!(r.clone.blobs_dropped, 0);
        assert!(!sibling(&repo, TMP_SUFFIX).exists());
        assert!(!sibling(&repo, OLD_SUFFIX).exists());
        assert!(kinora_root(&repo).is_dir());
    }

    #[test]
    fn repack_errors_when_tmp_dir_exists() {
        let (_tmp, repo) = setup();
        fs::create_dir_all(sibling(&repo, TMP_SUFFIX)).unwrap();
        let err = repack_repo(&repo, repack_params("t")).unwrap_err();
        assert!(matches!(err, RepackError::TempExists { .. }), "got: {err:?}");
    }

    #[test]
    fn repack_errors_when_old_dir_exists() {
        let (_tmp, repo) = setup();
        fs::create_dir_all(sibling(&repo, OLD_SUFFIX)).unwrap();
        let err = repack_repo(&repo, repack_params("t")).unwrap_err();
        assert!(matches!(err, RepackError::OldExists { .. }), "got: {err:?}");
    }

    #[test]
    fn repack_commits_pending_events_before_clone() {
        let (_tmp, repo) = setup();
        let kin = kinora_root(&repo);
        // Stage a kino but don't commit it manually — repack should commit first.
        store_kino(&kin, store_params("2026-04-20T01:00:00Z")).unwrap();
        let r = repack_repo(&repo, repack_params("2026-04-20T02:00:00Z")).unwrap();
        // At least one root committed a new version.
        let any_new_version = r.commits.iter().any(|c| c.new_version.is_some());
        assert!(any_new_version, "expected at least one root to commit a new version");
        // After swap, .kinora/ is the rebuilt dir — resolver should find the kino.
        let resolver = crate::resolve::Resolver::load(kinora_root(&repo)).unwrap();
        let resolved = resolver.resolve_by_name("hello").unwrap();
        assert_eq!(resolved.content, b"# hello\n");
    }

    #[test]
    fn repack_leaves_no_lingering_dirs_on_success() {
        let (_tmp, repo) = setup();
        store_kino(&kinora_root(&repo), store_params("2026-04-20T01:00:00Z")).unwrap();
        repack_repo(&repo, repack_params("2026-04-20T02:00:00Z")).unwrap();
        assert!(!sibling(&repo, TMP_SUFFIX).exists());
        assert!(!sibling(&repo, OLD_SUFFIX).exists());
    }

    #[test]
    fn repack_rewrites_legacy_filenames() {
        let (_tmp, repo) = setup();
        let kin = kinora_root(&repo);
        store_kino(&kin, store_params("2026-04-20T01:00:00Z")).unwrap();
        // Commit so the kino is reachable from a root head.
        commit_all(
            &kin,
            CommitParams {
                author: "yj".into(),
                provenance: "repack-test".into(),
                ts: "2026-04-20T01:30:00Z".into(),
            },
        )
        .unwrap();

        // Simulate a legacy extensionless filename in src: find any
        // blob that has a `.md` or `.styxl` extension and rename it to
        // its bare 64-hex form.
        let store = crate::paths::store_dir(&kin);
        let mut rewritten_any = false;
        for shard in fs::read_dir(&store).unwrap() {
            let shard = shard.unwrap();
            if !shard.file_type().unwrap().is_dir() {
                continue;
            }
            for entry in fs::read_dir(shard.path()).unwrap() {
                let entry = entry.unwrap();
                let name = entry.file_name().to_string_lossy().into_owned();
                if let Some((stem, _ext)) = name.split_once('.')
                    && stem.len() == 64
                    && stem.bytes().all(|b| b.is_ascii_hexdigit())
                {
                    let new_path = shard.path().join(stem);
                    fs::rename(entry.path(), &new_path).unwrap();
                    rewritten_any = true;
                    break;
                }
            }
            if rewritten_any {
                break;
            }
        }
        assert!(rewritten_any, "expected at least one blob to strip");

        let r = repack_repo(&repo, repack_params("2026-04-20T02:00:00Z")).unwrap();
        assert!(
            r.clone.filenames_rewritten >= 1,
            "expected legacy filename to be rewritten, got {}",
            r.clone.filenames_rewritten
        );
    }
}
