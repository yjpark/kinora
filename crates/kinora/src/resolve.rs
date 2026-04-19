use std::collections::{BTreeMap, HashMap, HashSet};
use std::str::FromStr;

use crate::event::Event;
use crate::hash::{Hash, HashParseError};
use crate::ledger::{Ledger, LedgerError};
use crate::store::{ContentStore, StoreError};

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error(transparent)]
    Ledger(#[from] LedgerError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("invalid hash `{value}`: {err}")]
    InvalidHash {
        value: String,
        #[source]
        err: HashParseError,
    },
    #[error("no kino found for `{query}`")]
    NotFound { query: String },
    #[error("name `{name}` is ambiguous; matches {} identities: {}", .ids.len(), .ids.join(", "))]
    AmbiguousName { name: String, ids: Vec<String> },
    #[error("identity {id} has {} heads; pass --version HASH or --all-heads", .heads.len())]
    MultipleHeads { id: String, heads: Vec<Event>, lineages: Vec<String> },
    #[error("identity {id} has no version with hash {version}")]
    VersionNotFound { id: String, version: String },
}

/// All events belonging to a single identity, with heads precomputed.
///
/// `heads` are events whose `hash` is not listed in any other same-identity
/// event's `parents`. Single head = no fork; multiple heads = fork.
#[derive(Debug, Clone)]
pub struct Identity {
    pub id: String,
    pub events: Vec<Event>,
    pub heads: Vec<Event>,
    /// Lineage shorthash for each event, in the same order as `events`.
    pub lineages: Vec<String>,
}

impl Identity {
    fn build(id: String, events_with_lineage: Vec<(String, Event)>) -> Self {
        let (lineages, events): (Vec<_>, Vec<_>) = events_with_lineage.into_iter().unzip();

        let referenced: HashSet<&str> = events
            .iter()
            .flat_map(|e| e.parents.iter().map(String::as_str))
            .collect();
        let heads: Vec<Event> = events
            .iter()
            .filter(|e| !referenced.contains(e.hash.as_str()))
            .cloned()
            .collect();
        Self { id, events, heads, lineages }
    }

    /// Lineage shorthash of a specific event within this identity, or None
    /// if `event_hash` does not belong to this identity.
    pub fn lineage_of(&self, event_hash: &str) -> Option<&str> {
        self.events
            .iter()
            .zip(self.lineages.iter())
            .find_map(|(e, l)| (e.hash == event_hash).then_some(l.as_str()))
    }
}

/// Result of a successful resolve.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub id: String,
    pub head: Event,
    pub content: Vec<u8>,
    pub lineage: String,
    pub all_heads: Vec<Event>,
}

pub struct Resolver {
    kinora_root: std::path::PathBuf,
    identities: HashMap<String, Identity>,
}

impl Resolver {
    /// Load all identities from every event recorded under `.kinora/`.
    /// Reads both the new staged layout (`staged/<ab>/<event-hash>.jsonl`) and the
    /// legacy per-lineage layout (`ledger/<lineage>.jsonl`) and unions them,
    /// deduping by event hash. Events are grouped by `id`; heads precomputed.
    #[fastrace::trace]
    pub fn load(kinora_root: impl Into<std::path::PathBuf>) -> Result<Self, ResolveError> {
        let kinora_root = kinora_root.into();
        let ledger = Ledger::new(&kinora_root);

        let mut seen: HashSet<String> = HashSet::new();
        let mut by_id: BTreeMap<String, Vec<(String, Event)>> = BTreeMap::new();

        // Legacy layout: one file per lineage, each with many events. The
        // lineage label is the filename shorthash — meaningful for legacy
        // forks disambiguated by HEAD.
        for (lineage, events) in ledger.read_all_lineages()? {
            for event in events {
                if !event.is_store_event() {
                    continue;
                }
                let eh = event.event_hash().map_err(LedgerError::from)?;
                if !seen.insert(eh.as_hex().to_owned()) {
                    continue;
                }
                by_id
                    .entry(event.id.clone())
                    .or_default()
                    .push((lineage.clone(), event));
            }
        }

        // Staged layout: one file per event. Per-event lineage label = event
        // hash shorthash.
        for event in ledger.read_all_events()? {
            if !event.is_store_event() {
                continue;
            }
            let eh = event.event_hash().map_err(LedgerError::from)?;
            if !seen.insert(eh.as_hex().to_owned()) {
                continue;
            }
            let lineage = eh.shorthash().to_owned();
            by_id
                .entry(event.id.clone())
                .or_default()
                .push((lineage, event));
        }

        let identities: HashMap<String, Identity> = by_id
            .into_iter()
            .map(|(id, events)| (id.clone(), Identity::build(id, events)))
            .collect();

        Ok(Self { kinora_root, identities })
    }

    pub fn identities(&self) -> &HashMap<String, Identity> {
        &self.identities
    }

    /// Resolve a kino by its identity hash. Returns an error if unknown.
    pub fn resolve_by_id(&self, id: &str) -> Result<Resolved, ResolveError> {
        let identity = self
            .identities
            .get(id)
            .ok_or_else(|| ResolveError::NotFound { query: id.to_owned() })?;
        self.pick_head(identity)
    }

    /// Resolve by a `metadata.name` value. Matches against the latest
    /// version of each identity. Errors on zero or multiple matches.
    pub fn resolve_by_name(&self, name: &str) -> Result<Resolved, ResolveError> {
        let matches: Vec<&Identity> = self
            .identities
            .values()
            .filter(|ident| {
                ident
                    .heads
                    .iter()
                    .any(|e| e.metadata.get("name").map(String::as_str) == Some(name))
            })
            .collect();
        match matches.as_slice() {
            [] => Err(ResolveError::NotFound { query: name.to_owned() }),
            [only] => self.pick_head(only),
            many => Err(ResolveError::AmbiguousName {
                name: name.to_owned(),
                ids: many.iter().map(|i| i.id.clone()).collect(),
            }),
        }
    }

    /// Return the content of a specific prior version of an identity.
    pub fn resolve_at_version(
        &self,
        id: &str,
        version: &str,
    ) -> Result<Resolved, ResolveError> {
        let identity = self
            .identities
            .get(id)
            .ok_or_else(|| ResolveError::NotFound { query: id.to_owned() })?;
        let event = identity
            .events
            .iter()
            .find(|e| e.hash == version)
            .ok_or_else(|| ResolveError::VersionNotFound {
                id: id.to_owned(),
                version: version.to_owned(),
            })?
            .clone();
        let hash = parse_hash(&event.hash)?;
        let content = ContentStore::new(&self.kinora_root).read(&hash)?;
        let lineage = identity
            .lineage_of(&event.hash)
            .unwrap_or("")
            .to_owned();
        Ok(Resolved {
            id: id.to_owned(),
            head: event,
            content,
            lineage,
            all_heads: identity.heads.clone(),
        })
    }

    fn pick_head(&self, identity: &Identity) -> Result<Resolved, ResolveError> {
        let head = match identity.heads.as_slice() {
            [] => return Err(ResolveError::NotFound { query: identity.id.clone() }),
            [only] => only.clone(),
            many => {
                if let Some(unique) = self.head_for_current_lineage(identity, many)? {
                    unique
                } else {
                    let lineages = many
                        .iter()
                        .map(|h| {
                            identity
                                .lineage_of(&h.hash)
                                .unwrap_or("?")
                                .to_owned()
                        })
                        .collect();
                    return Err(ResolveError::MultipleHeads {
                        id: identity.id.clone(),
                        heads: many.to_vec(),
                        lineages,
                    });
                }
            }
        };
        let hash = parse_hash(&head.hash)?;
        let content = ContentStore::new(&self.kinora_root).read(&hash)?;
        let lineage = identity
            .lineage_of(&head.hash)
            .unwrap_or("")
            .to_owned();
        Ok(Resolved {
            id: identity.id.clone(),
            head,
            content,
            lineage,
            all_heads: identity.heads.clone(),
        })
    }

    /// Branch-aware tiebreak: if HEAD points to a lineage and exactly one
    /// of the candidate heads lives in that lineage, return it. Otherwise
    /// Ok(None) so the caller reports a fork.
    fn head_for_current_lineage(
        &self,
        identity: &Identity,
        heads: &[Event],
    ) -> Result<Option<Event>, ResolveError> {
        let current = match Ledger::new(&self.kinora_root).current_lineage()? {
            Some(l) => l,
            None => return Ok(None),
        };
        let in_current: Vec<&Event> = heads
            .iter()
            .filter(|h| identity.lineage_of(&h.hash) == Some(current.as_str()))
            .collect();
        match in_current.as_slice() {
            [only] => Ok(Some((*only).clone())),
            _ => Ok(None),
        }
    }
}

fn parse_hash(value: &str) -> Result<Hash, ResolveError> {
    Hash::from_str(value).map_err(|err| ResolveError::InvalidHash {
        value: value.to_owned(),
        err,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::init::init;
    use crate::kino::{store_kino, StoreKinoParams};
    use crate::paths::kinora_root;
    use tempfile::TempDir;

    fn setup() -> (TempDir, std::path::PathBuf) {
        let tmp = TempDir::new().unwrap();
        init(tmp.path(), "https://example.com/x.git").unwrap();
        let root = kinora_root(tmp.path());
        (tmp, root)
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
    fn resolve_by_id_returns_birth_when_single_head() {
        let (_t, root) = setup();
        let stored = store_kino(&root, params("markdown", b"hello", "greet")).unwrap();
        let resolver = Resolver::load(&root).unwrap();
        let resolved = resolver.resolve_by_id(&stored.event.id).unwrap();
        assert_eq!(resolved.content, b"hello");
        assert_eq!(resolved.id, stored.event.id);
        assert_eq!(resolved.all_heads.len(), 1);
    }

    #[test]
    fn resolve_unknown_id_errors() {
        let (_t, root) = setup();
        let resolver = Resolver::load(&root).unwrap();
        let err = resolver.resolve_by_id("0".repeat(64).as_str()).unwrap_err();
        assert!(matches!(err, ResolveError::NotFound { .. }));
    }

    #[test]
    fn resolve_returns_latest_version_on_linear_history() {
        let (_t, root) = setup();
        let birth = store_kino(&root, params("markdown", b"v1", "doc")).unwrap();

        let mut p = params("markdown", b"v2", "doc");
        p.id = Some(birth.event.id.clone());
        p.parents = vec![birth.event.hash.clone()];
        p.ts = "2026-04-18T10:00:01Z".into();
        let v2 = store_kino(&root, p).unwrap();

        let resolver = Resolver::load(&root).unwrap();
        let resolved = resolver.resolve_by_id(&birth.event.id).unwrap();
        assert_eq!(resolved.content, b"v2");
        assert_eq!(resolved.head.hash, v2.event.hash);
    }

    #[test]
    fn multiple_heads_produce_fork_error() {
        let (_t, root) = setup();
        let birth = store_kino(&root, params("markdown", b"v1", "doc")).unwrap();

        // Two sibling versions off the same parent.
        let mut a = params("markdown", b"left", "doc");
        a.id = Some(birth.event.id.clone());
        a.parents = vec![birth.event.hash.clone()];
        a.ts = "2026-04-18T10:00:01Z".into();
        store_kino(&root, a).unwrap();

        let mut b = params("markdown", b"right", "doc");
        b.id = Some(birth.event.id.clone());
        b.parents = vec![birth.event.hash.clone()];
        b.ts = "2026-04-18T10:00:02Z".into();
        store_kino(&root, b).unwrap();

        let resolver = Resolver::load(&root).unwrap();
        let err = resolver.resolve_by_id(&birth.event.id).unwrap_err();
        assert!(matches!(err, ResolveError::MultipleHeads { .. }));
    }

    #[test]
    fn resolve_by_name_finds_unique_identity() {
        let (_t, root) = setup();
        store_kino(&root, params("markdown", b"hi", "greeting")).unwrap();
        store_kino(&root, params("markdown", b"bye", "farewell")).unwrap();

        let resolver = Resolver::load(&root).unwrap();
        let resolved = resolver.resolve_by_name("greeting").unwrap();
        assert_eq!(resolved.content, b"hi");
    }

    #[test]
    fn resolve_by_name_ambiguous_across_identities() {
        let (_t, root) = setup();
        store_kino(&root, params("markdown", b"a", "same-name")).unwrap();
        store_kino(&root, params("markdown", b"b", "same-name")).unwrap();

        let resolver = Resolver::load(&root).unwrap();
        let err = resolver.resolve_by_name("same-name").unwrap_err();
        assert!(matches!(err, ResolveError::AmbiguousName { .. }));
    }

    #[test]
    fn resolve_by_name_missing_errors() {
        let (_t, root) = setup();
        store_kino(&root, params("markdown", b"x", "other")).unwrap();
        let resolver = Resolver::load(&root).unwrap();
        let err = resolver.resolve_by_name("no-such").unwrap_err();
        assert!(matches!(err, ResolveError::NotFound { .. }));
    }

    #[test]
    fn resolve_at_version_returns_specific_prior() {
        let (_t, root) = setup();
        let birth = store_kino(&root, params("markdown", b"v1", "doc")).unwrap();

        let mut p = params("markdown", b"v2", "doc");
        p.id = Some(birth.event.id.clone());
        p.parents = vec![birth.event.hash.clone()];
        p.ts = "2026-04-18T10:00:01Z".into();
        store_kino(&root, p).unwrap();

        let resolver = Resolver::load(&root).unwrap();
        let resolved = resolver
            .resolve_at_version(&birth.event.id, &birth.event.hash)
            .unwrap();
        assert_eq!(resolved.content, b"v1");
        assert_eq!(resolved.head.hash, birth.event.hash);
    }

    #[test]
    fn resolve_at_version_rejects_unknown_hash() {
        let (_t, root) = setup();
        let birth = store_kino(&root, params("markdown", b"v1", "doc")).unwrap();
        let resolver = Resolver::load(&root).unwrap();
        let bogus = "0".repeat(64);
        let err = resolver
            .resolve_at_version(&birth.event.id, &bogus)
            .unwrap_err();
        assert!(matches!(err, ResolveError::VersionNotFound { .. }));
    }

    #[test]
    fn branch_aware_tiebreak_still_works_for_legacy_lineage_files() {
        // Under the staged-ledger layout, per-event files don't participate in
        // HEAD-based tiebreak (all events live in their own file, so none
        // shares a lineage with HEAD). The tiebreaker is retained only for
        // legacy `.kinora/ledger/<lineage>.jsonl` files so previously-written
        // data still disambiguates correctly. Simulate a legacy fork by
        // writing raw lineage files directly.
        let (_t, root) = setup();
        let ledger = Ledger::new(&root);
        let store = ContentStore::new(&root);
        store.ensure_layout().unwrap();
        ledger.ensure_layout().unwrap();

        // Birth + v2 sharing one legacy lineage file.
        let birth_hash = store.write("markdown", b"v1").unwrap();
        let birth = Event::new_store(
            "markdown".into(),
            birth_hash.as_hex().into(),
            birth_hash.as_hex().into(),
            vec![],
            "2026-04-18T10:00:00Z".into(),
            "yj".into(),
            "legacy".into(),
            BTreeMap::from([("name".into(), "doc".into())]),
        );
        let sh_a = ledger.mint_and_append(&birth).unwrap();

        let v2_hash = store.write("markdown", b"v2").unwrap();
        let v2 = Event::new_store(
            "markdown".into(),
            birth.id.clone(),
            v2_hash.as_hex().into(),
            vec![birth.hash.clone()],
            "2026-04-18T10:00:01Z".into(),
            "yj".into(),
            "legacy".into(),
            BTreeMap::from([("name".into(), "doc".into())]),
        );
        ledger.append_to_head(&v2).unwrap();

        // Sibling head in a separate legacy lineage file.
        std::fs::remove_file(crate::paths::head_path(&root)).unwrap();
        let sibling_hash = store.write("markdown", b"right").unwrap();
        let sibling = Event::new_store(
            "markdown".into(),
            birth.id.clone(),
            sibling_hash.as_hex().into(),
            vec![birth.hash.clone()],
            "2026-04-18T10:00:02Z".into(),
            "yj".into(),
            "legacy".into(),
            BTreeMap::from([("name".into(), "doc".into())]),
        );
        ledger.mint_and_append(&sibling).unwrap();

        // Point HEAD at the first (A-lineage) file.
        ledger.set_head(&sh_a).unwrap();

        let resolver = Resolver::load(&root).unwrap();
        let resolved = resolver.resolve_by_id(&birth.id).unwrap();
        // v2 lives in the HEAD lineage, so the tiebreaker picks it.
        assert_eq!(resolved.content, b"v2");
        assert_eq!(resolved.head.hash, v2.hash);
        assert_eq!(resolved.all_heads.len(), 2);
    }

    #[test]
    fn resolver_groups_events_across_staged_and_legacy_stores() {
        // Two independent identities — one new (via store_kino → staged), one
        // legacy (hand-written into `ledger/`). Resolver::load must surface
        // both.
        let (_t, root) = setup();
        let a = store_kino(&root, params("markdown", b"hi", "a")).unwrap();

        // Legacy: write a birth event directly to a new lineage file.
        let ledger = Ledger::new(&root);
        let store = ContentStore::new(&root);
        let legacy_hash = store.write("markdown", b"bye").unwrap();
        let legacy_event = Event::new_store(
            "markdown".into(),
            legacy_hash.as_hex().into(),
            legacy_hash.as_hex().into(),
            vec![],
            "2026-04-18T10:00:01Z".into(),
            "yj".into(),
            "legacy".into(),
            BTreeMap::from([("name".into(), "b".into())]),
        );
        std::fs::remove_file(crate::paths::head_path(&root)).ok();
        ledger.mint_and_append(&legacy_event).unwrap();

        let resolver = Resolver::load(&root).unwrap();
        assert!(resolver.identities().contains_key(&a.event.id));
        assert!(resolver.identities().contains_key(&legacy_event.id));
    }

    #[test]
    fn resolver_dedups_when_same_event_lives_in_both_staged_and_legacy() {
        // During the transition, the same logical event may appear in both
        // layouts — e.g. a legacy lineage file still on disk alongside a staged
        // event file. Resolver must dedup by event hash so heads/parents don't
        // double-count.
        let (_t, root) = setup();
        let ledger = Ledger::new(&root);
        let store = ContentStore::new(&root);
        store.ensure_layout().unwrap();
        ledger.ensure_layout().unwrap();

        let content_hash = store.write("markdown", b"same").unwrap();
        let event = Event::new_store(
            "markdown".into(),
            content_hash.as_hex().into(),
            content_hash.as_hex().into(),
            vec![],
            "2026-04-18T10:00:00Z".into(),
            "yj".into(),
            "dual-layout".into(),
            BTreeMap::from([("name".into(), "dual".into())]),
        );

        ledger.mint_and_append(&event).unwrap();
        let (_, was_new) = ledger.write_event(&event).unwrap();
        assert!(was_new, "staged write should have created a new file");

        let resolver = Resolver::load(&root).unwrap();
        let identity = resolver.identities().get(&event.id).expect("identity present");
        assert_eq!(identity.events.len(), 1, "duplicate event not deduped");
        assert_eq!(identity.heads.len(), 1);
    }

    #[test]
    fn fork_from_staged_events_produces_multiple_heads() {
        // Under the staged-ledger layout, HEAD-based tiebreak no longer applies
        // to new events (each lives in its own file). Forks therefore always
        // surface as `MultipleHeads`, which callers must resolve explicitly.
        let (_t, root) = setup();
        let birth = store_kino(&root, params("markdown", b"v1", "doc")).unwrap();

        for (content, ts) in [
            (b"left" as &[u8], "2026-04-18T10:00:01Z"),
            (b"right", "2026-04-18T10:00:02Z"),
        ] {
            let mut p = params("markdown", content, "doc");
            p.id = Some(birth.event.id.clone());
            p.parents = vec![birth.event.hash.clone()];
            p.ts = ts.into();
            store_kino(&root, p).unwrap();
        }

        let resolver = Resolver::load(&root).unwrap();
        let err = resolver.resolve_by_id(&birth.event.id).unwrap_err();
        assert!(matches!(err, ResolveError::MultipleHeads { .. }));
    }

    #[test]
    fn resolver_ignores_non_store_events() {
        // Hand-forge a non-store (e.g. assign-track) event and write it to
        // the staged ledger. Resolver::load must skip it so the identity it
        // carries doesn't pollute the content graph.
        let (_t, root) = setup();
        let ledger = Ledger::new(&root);
        ledger.ensure_layout().unwrap();

        let forged_id = "cc".repeat(32);
        let forged = Event {
            event_kind: "assign".into(),
            kind: "assign".into(),
            id: forged_id.clone(),
            hash: "dd".repeat(32),
            parents: vec![],
            ts: "2026-04-18T10:00:00Z".into(),
            author: "yj".into(),
            provenance: "test".into(),
            metadata: BTreeMap::new(),
        };
        ledger.write_event(&forged).unwrap();

        let resolver = Resolver::load(&root).unwrap();
        assert!(
            !resolver.identities().contains_key(&forged_id),
            "non-store event must not surface as a resolver identity"
        );
    }

    #[test]
    fn identity_name_is_read_from_latest_head_not_birth() {
        let (_t, root) = setup();
        let birth = store_kino(&root, params("markdown", b"v1", "old-name")).unwrap();

        let mut p = params("markdown", b"v2", "new-name");
        p.id = Some(birth.event.id.clone());
        p.parents = vec![birth.event.hash.clone()];
        p.ts = "2026-04-18T10:00:01Z".into();
        store_kino(&root, p).unwrap();

        let resolver = Resolver::load(&root).unwrap();
        // Renaming takes effect for name lookup.
        assert!(resolver.resolve_by_name("new-name").is_ok());
        assert!(matches!(
            resolver.resolve_by_name("old-name").unwrap_err(),
            ResolveError::NotFound { .. }
        ));
    }
}
