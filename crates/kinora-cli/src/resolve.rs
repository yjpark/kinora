use std::io::{self, Write};
use std::path::Path;
use std::str::FromStr;

use kinora::event::Event;
use kinora::hash::{Hash, SHORTHASH_LEN};
use kinora::paths::kinora_root;
use kinora::resolve::{ResolveError, Resolved, Resolver};

use crate::common::{find_repo_root, CliError};

pub struct ResolveRunArgs {
    pub name_or_id: String,
    pub version: Option<String>,
    pub all_heads: bool,
}

/// Outcome of a resolve run: either a single resolved kino (content to
/// write to stdout) or a list of heads (rendered as a fork report).
#[derive(Debug)]
pub enum ResolveOutcome {
    Content(Box<Resolved>),
    AllHeads { id: String, heads: Vec<Event> },
}

pub fn run_resolve(cwd: &Path, args: ResolveRunArgs) -> Result<ResolveOutcome, CliError> {
    let repo_root = find_repo_root(cwd)?;
    let kin_root = kinora_root(&repo_root);

    let resolver = Resolver::load(&kin_root)?;

    if let Some(version) = args.version.as_deref() {
        // --version requires an identity hash up front: names are matched
        // against the *current* head's metadata, so resolving a name at a
        // prior version isn't meaningful without first resolving the id.
        let id = if looks_like_hash(&args.name_or_id) {
            args.name_or_id.clone()
        } else {
            resolver.resolve_by_name(&args.name_or_id)?.id
        };
        let resolved = resolver.resolve_at_version(&id, version)?;
        return Ok(ResolveOutcome::Content(Box::new(resolved)));
    }

    match resolve_initial(&resolver, &args.name_or_id) {
        Ok(resolved) => Ok(ResolveOutcome::Content(Box::new(resolved))),
        Err(CliError::Resolve(ResolveError::MultipleHeads { id, heads, .. })) if args.all_heads => {
            Ok(ResolveOutcome::AllHeads { id, heads })
        }
        Err(e) => Err(e),
    }
}

fn resolve_initial(resolver: &Resolver, name_or_id: &str) -> Result<Resolved, CliError> {
    if looks_like_hash(name_or_id) {
        Ok(resolver.resolve_by_id(name_or_id)?)
    } else {
        Ok(resolver.resolve_by_name(name_or_id)?)
    }
}

fn looks_like_hash(s: &str) -> bool {
    Hash::from_str(s).is_ok()
}

fn short(hex: &str) -> &str {
    &hex[..hex.len().min(SHORTHASH_LEN)]
}

/// Render a MultipleHeads error as the actionable fork report from the
/// bean spec. Written generically over any writer for testing.
pub fn render_fork_report<W: Write>(
    w: &mut W,
    name_or_id: &str,
    id: &str,
    heads: &[Event],
    lineages: &[String],
) -> io::Result<()> {
    writeln!(
        w,
        "kino `{name_or_id}` (id: {short}…) has {n} heads:",
        short = short(id),
        n = heads.len()
    )?;
    for (head, lineage) in heads.iter().zip(lineages.iter()) {
        writeln!(
            w,
            "  - {short}… (lineage {lineage}, {} @ {})",
            head.author, head.ts,
            short = short(&head.hash)
        )?;
    }
    writeln!(w)?;
    let parents = heads
        .iter()
        .map(|h| h.hash.as_str())
        .collect::<Vec<_>>()
        .join(",");
    let kind = heads.first().map(|h| h.kind.as_str()).unwrap_or("<kind>");
    writeln!(w, "Reconcile via one of:")?;
    writeln!(
        w,
        "  - merge:     kinora store {kind} --id {id} --parents {parents} <content>"
    )?;
    writeln!(w, "  - linearize: pick one head; write as new version with both as parents")?;
    writeln!(w, "  - keep-both: append metadata event introducing variant names")?;
    writeln!(w, "  - detach:    treat one head as new identity")?;
    Ok(())
}

/// Render an `--all-heads` listing: one line per head with its hash,
/// lineage, author and timestamp. No reconcile block (the caller opted
/// in to seeing the forks).
pub fn render_all_heads<W: Write>(
    w: &mut W,
    id: &str,
    heads: &[Event],
    lineages: &[String],
) -> io::Result<()> {
    writeln!(w, "id: {id}")?;
    writeln!(w, "heads ({}):", heads.len())?;
    for (head, lineage) in heads.iter().zip(lineages.iter()) {
        writeln!(
            w,
            "  - {hash} (lineage {lineage}, {} @ {})",
            head.author, head.ts,
            hash = head.hash
        )?;
    }
    Ok(())
}

/// Lineage shorthashes for each head, using the resolver's identity map.
/// Missing entries are rendered as "?" so the caller never sees a panic.
pub fn head_lineages(resolver: &Resolver, id: &str, heads: &[Event]) -> Vec<String> {
    let identity = match resolver.identities().get(id) {
        Some(i) => i,
        None => return heads.iter().map(|_| "?".to_owned()).collect(),
    };
    heads
        .iter()
        .map(|h| identity.lineage_of(&h.hash).unwrap_or("?").to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use kinora::event::Event;
    use kinora::init::init;
    use kinora::kino::{store_kino, StoreKinoParams};
    use std::collections::BTreeMap;
    use tempfile::TempDir;

    fn repo() -> TempDir {
        let tmp = TempDir::new().unwrap();
        init(tmp.path(), "https://example.com/x.git").unwrap();
        tmp
    }

    fn params(kind: &str, content: &[u8], name: &str) -> StoreKinoParams {
        StoreKinoParams {
            kind: kind.into(),
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
    fn resolve_by_id_returns_content() {
        let tmp = repo();
        let stored = store_kino(&kinora_root(tmp.path()), params("markdown", b"hello", "doc"))
            .unwrap();
        let args = ResolveRunArgs {
            name_or_id: stored.event.id.clone(),
            version: None,
            all_heads: false,
        };
        let outcome = run_resolve(tmp.path(), args).unwrap();
        match outcome {
            ResolveOutcome::Content(r) => assert_eq!(r.content, b"hello"),
            _ => panic!("expected content"),
        }
    }

    #[test]
    fn resolve_by_name_returns_content() {
        let tmp = repo();
        store_kino(&kinora_root(tmp.path()), params("markdown", b"hi", "greet")).unwrap();
        let args = ResolveRunArgs {
            name_or_id: "greet".into(),
            version: None,
            all_heads: false,
        };
        let outcome = run_resolve(tmp.path(), args).unwrap();
        match outcome {
            ResolveOutcome::Content(r) => assert_eq!(r.content, b"hi"),
            _ => panic!("expected content"),
        }
    }

    #[test]
    fn unknown_name_errors_with_not_found() {
        let tmp = repo();
        let args = ResolveRunArgs {
            name_or_id: "nobody".into(),
            version: None,
            all_heads: false,
        };
        let err = run_resolve(tmp.path(), args).unwrap_err();
        assert!(matches!(
            err,
            CliError::Resolve(ResolveError::NotFound { .. })
        ));
    }

    #[test]
    fn resolve_at_version_by_id() {
        let tmp = repo();
        let root = kinora_root(tmp.path());
        let birth = store_kino(&root, params("markdown", b"v1", "doc")).unwrap();

        let mut p = params("markdown", b"v2", "doc");
        p.id = Some(birth.event.id.clone());
        p.parents = vec![birth.event.hash.clone()];
        p.ts = "2026-04-18T10:00:01Z".into();
        store_kino(&root, p).unwrap();

        let args = ResolveRunArgs {
            name_or_id: birth.event.id.clone(),
            version: Some(birth.event.hash.clone()),
            all_heads: false,
        };
        let outcome = run_resolve(tmp.path(), args).unwrap();
        match outcome {
            ResolveOutcome::Content(r) => assert_eq!(r.content, b"v1"),
            _ => panic!("expected content"),
        }
    }

    #[test]
    fn resolve_at_version_by_name_resolves_name_first() {
        let tmp = repo();
        let root = kinora_root(tmp.path());
        let birth = store_kino(&root, params("markdown", b"v1", "doc")).unwrap();

        let mut p = params("markdown", b"v2", "doc");
        p.id = Some(birth.event.id.clone());
        p.parents = vec![birth.event.hash.clone()];
        p.ts = "2026-04-18T10:00:01Z".into();
        store_kino(&root, p).unwrap();

        let args = ResolveRunArgs {
            name_or_id: "doc".into(),
            version: Some(birth.event.hash.clone()),
            all_heads: false,
        };
        let outcome = run_resolve(tmp.path(), args).unwrap();
        match outcome {
            ResolveOutcome::Content(r) => assert_eq!(r.content, b"v1"),
            _ => panic!("expected content"),
        }
    }

    #[test]
    fn fork_errors_without_all_heads() {
        let tmp = repo();
        let root = kinora_root(tmp.path());
        let birth = store_kino(&root, params("markdown", b"v1", "doc")).unwrap();

        for (content, ts) in [(b"left" as &[u8], "2026-04-18T10:00:01Z"), (b"right", "2026-04-18T10:00:02Z")] {
            let mut p = params("markdown", content, "doc");
            p.id = Some(birth.event.id.clone());
            p.parents = vec![birth.event.hash.clone()];
            p.ts = ts.into();
            store_kino(&root, p).unwrap();
        }

        // HEAD is no longer written by the hot ledger, but older workspaces
        // may still have it — remove best-effort so the legacy tiebreak
        // cannot accidentally resolve the fork.
        let _ = std::fs::remove_file(kinora::paths::head_path(&root));

        let args = ResolveRunArgs {
            name_or_id: birth.event.id.clone(),
            version: None,
            all_heads: false,
        };
        let err = run_resolve(tmp.path(), args).unwrap_err();
        assert!(matches!(
            err,
            CliError::Resolve(ResolveError::MultipleHeads { .. })
        ));
    }

    #[test]
    fn fork_returns_all_heads_when_flag_set() {
        let tmp = repo();
        let root = kinora_root(tmp.path());
        let birth = store_kino(&root, params("markdown", b"v1", "doc")).unwrap();

        for (content, ts) in [(b"left" as &[u8], "2026-04-18T10:00:01Z"), (b"right", "2026-04-18T10:00:02Z")] {
            let mut p = params("markdown", content, "doc");
            p.id = Some(birth.event.id.clone());
            p.parents = vec![birth.event.hash.clone()];
            p.ts = ts.into();
            store_kino(&root, p).unwrap();
        }
        let _ = std::fs::remove_file(kinora::paths::head_path(&root));

        let args = ResolveRunArgs {
            name_or_id: birth.event.id.clone(),
            version: None,
            all_heads: true,
        };
        let outcome = run_resolve(tmp.path(), args).unwrap();
        match outcome {
            ResolveOutcome::AllHeads { heads, .. } => assert_eq!(heads.len(), 2),
            _ => panic!("expected all-heads"),
        }
    }

    #[test]
    fn fork_report_rendering_has_expected_shape() {
        let heads = vec![
            Event {
                kind: "markdown".into(),
                id: "a".repeat(64),
                hash: "b".repeat(64),
                parents: vec![],
                ts: "2026-04-10T00:00:00Z".into(),
                author: "yj".into(),
                provenance: "test".into(),
                metadata: BTreeMap::new(),
            },
            Event {
                kind: "markdown".into(),
                id: "a".repeat(64),
                hash: "c".repeat(64),
                parents: vec![],
                ts: "2026-04-12T00:00:00Z".into(),
                author: "yj".into(),
                provenance: "test".into(),
                metadata: BTreeMap::new(),
            },
        ];
        let lineages = vec!["lllll1".to_string(), "lllll2".to_string()];
        let mut buf = Vec::new();
        render_fork_report(&mut buf, "content-addressing", &"a".repeat(64), &heads, &lineages).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("content-addressing"));
        assert!(s.contains("has 2 heads"));
        assert!(s.contains("bbbbbbbb…"));
        assert!(s.contains("cccccccc…"));
        assert!(s.contains("lineage lllll1"));
        assert!(s.contains("yj @ 2026-04-10T00:00:00Z"));
        assert!(s.contains("Reconcile via"));
        assert!(s.contains("kinora store markdown --id"));
    }

    #[test]
    fn all_heads_rendering_lists_each_head() {
        let heads = vec![
            Event {
                kind: "markdown".into(),
                id: "a".repeat(64),
                hash: "b".repeat(64),
                parents: vec![],
                ts: "2026-04-10T00:00:00Z".into(),
                author: "yj".into(),
                provenance: "test".into(),
                metadata: BTreeMap::new(),
            },
            Event {
                kind: "markdown".into(),
                id: "a".repeat(64),
                hash: "c".repeat(64),
                parents: vec![],
                ts: "2026-04-12T00:00:00Z".into(),
                author: "yj".into(),
                provenance: "test".into(),
                metadata: BTreeMap::new(),
            },
        ];
        let lineages = vec!["lllll1".to_string(), "lllll2".to_string()];
        let mut buf = Vec::new();
        render_all_heads(&mut buf, &"a".repeat(64), &heads, &lineages).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains(&format!("id: {}", "a".repeat(64))));
        assert!(s.contains("heads (2)"));
        assert!(s.contains(&"b".repeat(64)));
        assert!(s.contains(&"c".repeat(64)));
        assert!(s.contains("lllll1"));
        assert!(s.contains("lllll2"));
    }

    #[test]
    fn errors_when_run_outside_kinora_repo() {
        let tmp = TempDir::new().unwrap();
        let args = ResolveRunArgs {
            name_or_id: "x".into(),
            version: None,
            all_heads: false,
        };
        let err = run_resolve(tmp.path(), args).unwrap_err();
        assert!(matches!(err, CliError::NotInKinoraRepo { .. }));
    }
}
