use std::path::Path;
use std::str::FromStr;

use kinora::assign::{write_assign, AssignEvent};
use kinora::author::resolve_author_from_git;
use kinora::hash::Hash;
use kinora::paths::kinora_root;
use kinora::resolve::Resolver;

use crate::common::{find_repo_root, parse_parents, CliError};

/// Inputs to the `assign` subcommand — mirrors the figue-parsed fields so
/// the runner is pure (no argv, no env) and easy to unit-test.
pub struct AssignRunArgs {
    pub kino: String,
    pub root: String,
    pub resolves: Option<String>,
    pub author: Option<String>,
    pub provenance: Option<String>,
}

/// Outcome of `kinora assign` — the written assign event's hash and
/// whether a new hot file was introduced (false on idempotent re-assign).
#[derive(Debug)]
pub struct AssignRunResult {
    pub kino_id: String,
    pub target_root: String,
    pub event_hash: Hash,
    pub was_new: bool,
}

pub fn run_assign(cwd: &Path, args: AssignRunArgs) -> Result<AssignRunResult, CliError> {
    let repo_root = find_repo_root(cwd)?;
    let kin_root = kinora_root(&repo_root);

    if args.root.is_empty() {
        return Err(CliError::EmptyRoot);
    }

    let kino_id = resolve_kino_id(&kin_root, &args.kino)?;

    let supersedes = parse_parents(args.resolves.as_deref());

    let author = match args.author {
        Some(a) => a,
        None => resolve_author_from_git(&repo_root).ok_or(CliError::AuthorUnresolved)?,
    };

    let ts = jiff::Timestamp::now().to_string();
    let provenance = args.provenance.unwrap_or_else(|| "assign".to_owned());

    let assign = AssignEvent {
        kino_id: kino_id.clone(),
        target_root: args.root.clone(),
        supersedes,
        author,
        ts,
        provenance,
    };

    let (event_hash, was_new) = write_assign(&kin_root, &assign)?;
    Ok(AssignRunResult {
        kino_id,
        target_root: args.root,
        event_hash,
        was_new,
    })
}

/// Resolve a kino reference (either a 64-hex id or a metadata `name`) to
/// its identity hash. Delegates to the standard resolver so names follow
/// the same precedence rules as `kinora resolve`.
fn resolve_kino_id(kin_root: &Path, kino: &str) -> Result<String, CliError> {
    let resolver = Resolver::load(kin_root)?;
    if Hash::from_str(kino).is_ok() {
        let resolved = resolver.resolve_by_id(kino)?;
        Ok(resolved.id)
    } else {
        let resolved = resolver.resolve_by_name(kino)?;
        Ok(resolved.id)
    }
}

/// One-line human summary printed after `kinora assign` succeeds.
pub fn format_assign_summary(r: &AssignRunResult) -> String {
    let suffix = if r.was_new { " (new event)" } else { "" };
    format!(
        "assigned kino={} root={} event={}{}",
        r.kino_id,
        r.target_root,
        r.event_hash.shorthash(),
        suffix,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use kinora::assign::{AssignEvent, EVENT_KIND_ASSIGN, META_TARGET_ROOT};
    use kinora::init::init;
    use kinora::kino::{store_kino, StoreKinoParams};
    use kinora::ledger::Ledger;
    use kinora::resolve::ResolveError;
    use std::collections::BTreeMap;
    use tempfile::TempDir;

    fn repo() -> TempDir {
        let tmp = TempDir::new().unwrap();
        init(tmp.path(), "https://example.com/x.git").unwrap();
        tmp
    }

    fn seed_kino(tmp: &TempDir, content: &[u8], name: &str) -> String {
        let root = kinora_root(tmp.path());
        let params = StoreKinoParams {
            kind: "markdown".into(),
            content: content.to_vec(),
            author: "yj".into(),
            provenance: "seed".into(),
            ts: "2026-04-19T10:00:00Z".into(),
            metadata: BTreeMap::from([("name".into(), name.into())]),
            id: None,
            parents: vec![],
        };
        store_kino(&root, params).unwrap().event.id
    }

    fn base_args(kino: &str, root: &str) -> AssignRunArgs {
        AssignRunArgs {
            kino: kino.into(),
            root: root.into(),
            resolves: None,
            author: Some("YJ".into()),
            provenance: None,
        }
    }

    #[test]
    fn assign_by_name_writes_event_with_correct_kino_id() {
        let tmp = repo();
        let id = seed_kino(&tmp, b"hello", "doc");

        let r = run_assign(tmp.path(), base_args("doc", "main")).unwrap();
        assert_eq!(r.kino_id, id);
        assert_eq!(r.target_root, "main");
        assert!(r.was_new);
    }

    #[test]
    fn assign_by_id_writes_event_with_correct_kino_id() {
        let tmp = repo();
        let id = seed_kino(&tmp, b"hello", "doc");

        let r = run_assign(tmp.path(), base_args(&id, "main")).unwrap();
        assert_eq!(r.kino_id, id);
    }

    #[test]
    fn assign_writes_exactly_one_event_readable_as_assign() {
        let tmp = repo();
        seed_kino(&tmp, b"x", "doc");

        run_assign(tmp.path(), base_args("doc", "main")).unwrap();

        let events = Ledger::new(kinora_root(tmp.path()))
            .read_all_events()
            .unwrap();
        let assigns: Vec<_> = events
            .iter()
            .filter(|e| e.event_kind == EVENT_KIND_ASSIGN)
            .collect();
        assert_eq!(assigns.len(), 1);
        assert_eq!(assigns[0].metadata.get(META_TARGET_ROOT).unwrap(), "main");
    }

    #[test]
    fn write_assign_is_idempotent_on_identical_inputs() {
        // `run_assign` itself can't be used to prove idempotency because it
        // stamps `ts` from `jiff::Timestamp::now()`, so two back-to-back calls
        // produce different event hashes. Confirm the CLI-resolved kino_id
        // round-trips into a stable assign event by calling the library twice
        // with a fixed ts.
        let tmp = repo();
        seed_kino(&tmp, b"x", "doc");

        let a = AssignEvent {
            kino_id: seed_kino_id(&tmp, "doc"),
            target_root: "main".into(),
            supersedes: vec![],
            author: "YJ".into(),
            ts: "2026-04-19T10:05:00Z".into(),
            provenance: "test".into(),
        };
        let (h1, new1) = write_assign(&kinora_root(tmp.path()), &a).unwrap();
        let (h2, new2) = write_assign(&kinora_root(tmp.path()), &a).unwrap();
        assert_eq!(h1, h2);
        assert!(new1);
        assert!(!new2);
    }

    fn seed_kino_id(tmp: &TempDir, name: &str) -> String {
        let resolver = Resolver::load(kinora_root(tmp.path())).unwrap();
        resolver.resolve_by_name(name).unwrap().id
    }

    #[test]
    fn assign_resolves_flag_populates_supersedes() {
        let tmp = repo();
        seed_kino(&tmp, b"x", "doc");

        let prior_a = Hash::of_content(b"prior-a").as_hex().to_owned();
        let prior_b = Hash::of_content(b"prior-b").as_hex().to_owned();

        let mut args = base_args("doc", "main");
        args.resolves = Some(format!("{prior_a},{prior_b}"));
        run_assign(tmp.path(), args).unwrap();

        let events = Ledger::new(kinora_root(tmp.path()))
            .read_all_events()
            .unwrap();
        let assign_ev = events
            .iter()
            .find(|e| e.event_kind == EVENT_KIND_ASSIGN)
            .unwrap();
        assert_eq!(assign_ev.parents, vec![prior_a, prior_b]);
    }

    #[test]
    fn assign_empty_root_rejected() {
        let tmp = repo();
        seed_kino(&tmp, b"x", "doc");
        let err = run_assign(tmp.path(), base_args("doc", "")).unwrap_err();
        assert!(matches!(err, CliError::EmptyRoot), "got {err:?}");
    }

    #[test]
    fn assign_unknown_kino_name_errors_with_not_found() {
        let tmp = repo();
        // No seed — resolver can't find "nobody".
        let err = run_assign(tmp.path(), base_args("nobody", "main")).unwrap_err();
        assert!(
            matches!(err, CliError::Resolve(ResolveError::NotFound { .. })),
            "got {err:?}"
        );
    }

    #[test]
    fn assign_errors_outside_kinora_repo() {
        let tmp = TempDir::new().unwrap();
        let err = run_assign(tmp.path(), base_args("doc", "main")).unwrap_err();
        assert!(matches!(err, CliError::NotInKinoraRepo { .. }));
    }

    #[test]
    fn assign_author_unresolved_when_flag_missing_and_no_git_name() {
        let tmp = repo();
        seed_kino(&tmp, b"x", "doc");
        let mut args = base_args("doc", "main");
        args.author = None;
        let err = run_assign(tmp.path(), args).unwrap_err();
        assert!(matches!(err, CliError::AuthorUnresolved), "got {err:?}");
    }

    #[test]
    fn assign_provenance_defaults_to_assign_when_omitted() {
        let tmp = repo();
        seed_kino(&tmp, b"x", "doc");
        run_assign(tmp.path(), base_args("doc", "main")).unwrap();
        let events = Ledger::new(kinora_root(tmp.path()))
            .read_all_events()
            .unwrap();
        let assign_ev = events
            .iter()
            .find(|e| e.event_kind == EVENT_KIND_ASSIGN)
            .unwrap();
        assert_eq!(assign_ev.provenance, "assign");
    }

    #[test]
    fn format_summary_has_expected_shape() {
        let r = AssignRunResult {
            kino_id: "aa".repeat(32),
            target_root: "main".into(),
            event_hash: Hash::of_content(b"x"),
            was_new: true,
        };
        let s = format_assign_summary(&r);
        assert!(s.contains("assigned kino="), "got {s}");
        assert!(s.contains("root=main"));
        assert!(s.contains(" (new event)"));
    }

    #[test]
    fn format_summary_omits_suffix_on_idempotent_assign() {
        let r = AssignRunResult {
            kino_id: "aa".repeat(32),
            target_root: "main".into(),
            event_hash: Hash::of_content(b"x"),
            was_new: false,
        };
        let s = format_assign_summary(&r);
        assert!(!s.contains("(new"), "got {s}");
    }
}
