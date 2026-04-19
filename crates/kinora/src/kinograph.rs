//! Kinograph composition format: `kind: kinograph` content is a styx
//! document whose top level is `entries (…)`. Each entry points at
//! another kino by id (authoritative) plus optional hints: a `name`
//! (re-checked against current metadata on render), a `pin` (freeze
//! this reference to a specific content hash), and a human-readable
//! `note` (rendered as a leading blockquote).
//!
//! Empty strings encode absent fields. facet-styx can't round-trip
//! `Option<String>` reliably (it serializes `None` as `@` but its own
//! parser rejects that), so the struct uses `String` with `""` as the
//! sentinel for absent. See the `entry_opt_accessors` helpers below
//! if an Option-flavored view is needed.
use facet::Facet;

use crate::hash::Hash;
use crate::resolve::{ResolveError, Resolver};
use std::str::FromStr;

#[derive(Facet, Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    #[facet(default)]
    pub name: String,
    #[facet(default)]
    pub pin: String,
    #[facet(default)]
    pub note: String,
}

impl Entry {
    pub fn with_id(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: String::new(),
            pin: String::new(),
            note: String::new(),
        }
    }

    pub fn name_opt(&self) -> Option<&str> {
        (!self.name.is_empty()).then_some(self.name.as_str())
    }

    pub fn pin_opt(&self) -> Option<&str> {
        (!self.pin.is_empty()).then_some(self.pin.as_str())
    }

    pub fn note_opt(&self) -> Option<&str> {
        (!self.note.is_empty()).then_some(self.note.as_str())
    }
}

#[derive(Facet, Debug, Clone, PartialEq, Eq)]
pub struct Kinograph {
    pub entries: Vec<Entry>,
}

#[derive(Debug, thiserror::Error)]
pub enum KinographError {
    #[error("failed to parse kinograph: {0}")]
    Parse(String),
    #[error("failed to serialize kinograph: {0}")]
    Serialize(String),
    #[error("invalid entry [{idx}]: {reason}")]
    InvalidEntry { idx: usize, reason: String },
    #[error(transparent)]
    Resolve(#[from] ResolveError),
    #[error("kinograph content is not valid UTF-8: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
    #[error("entry [{idx}]: name `{name}` is ambiguous; matches {}: {}", .ids.len(), .ids.join(", "))]
    AmbiguousName { idx: usize, name: String, ids: Vec<String> },
    #[error("entry [{idx}]: no kino found for name `{name}`")]
    NameNotFound { idx: usize, name: String },
}

impl Kinograph {
    pub fn parse(bytes: &[u8]) -> Result<Self, KinographError> {
        let s = std::str::from_utf8(bytes)
            .map_err(|e| KinographError::Parse(e.to_string()))?;
        Self::parse_str(s)
    }

    pub fn parse_str(input: &str) -> Result<Self, KinographError> {
        let parsed: Kinograph = facet_styx::from_str(input)
            .map_err(|e| KinographError::Parse(e.to_string()))?;
        for (idx, entry) in parsed.entries.iter().enumerate() {
            validate_entry(idx, entry)?;
        }
        Ok(parsed)
    }

    pub fn to_styx(&self) -> Result<String, KinographError> {
        facet_styx::to_string(self).map_err(|e| KinographError::Serialize(e.to_string()))
    }

    /// For each entry whose `id` is not a valid 64-hex hash, treat it
    /// as a name reference and resolve against `resolver`. The entry's
    /// `name` hint (if any) is checked first; otherwise the id slot is
    /// used as the lookup name. Errors on ambiguous or missing names.
    pub fn resolve_names(mut self, resolver: &Resolver) -> Result<Self, KinographError> {
        for (idx, entry) in self.entries.iter_mut().enumerate() {
            if Hash::from_str(&entry.id).is_ok() {
                continue;
            }
            let lookup = if entry.name.is_empty() {
                entry.id.clone()
            } else {
                entry.name.clone()
            };
            match resolver.resolve_by_name(&lookup) {
                Ok(r) => {
                    if entry.name.is_empty() {
                        entry.name = lookup.clone();
                    }
                    entry.id = r.id;
                }
                Err(ResolveError::NotFound { .. }) => {
                    return Err(KinographError::NameNotFound { idx, name: lookup });
                }
                Err(ResolveError::AmbiguousName { name, ids }) => {
                    return Err(KinographError::AmbiguousName { idx, name, ids });
                }
                Err(e) => return Err(KinographError::Resolve(e)),
            }
        }
        Ok(self)
    }

    /// Render to a markdown string by fetching each referenced kino's
    /// content. A `pin` value, if set, selects a specific prior version;
    /// otherwise the current head is used. Per-entry `note` becomes a
    /// leading blockquote. Entries are separated by a blank line.
    pub fn render(&self, resolver: &Resolver) -> Result<String, KinographError> {
        let mut out = String::new();
        for entry in &self.entries {
            let resolved = match entry.pin_opt() {
                Some(pin) => resolver.resolve_at_version(&entry.id, pin)?,
                None => resolver.resolve_by_id(&entry.id)?,
            };
            if let Some(note) = entry.note_opt() {
                for line in note.lines() {
                    out.push_str("> ");
                    out.push_str(line);
                    out.push('\n');
                }
                out.push('\n');
            }
            let body = String::from_utf8(resolved.content)?;
            out.push_str(&body);
            if !out.ends_with('\n') {
                out.push('\n');
            }
            out.push('\n');
        }
        while out.ends_with("\n\n") {
            out.pop();
        }
        Ok(out)
    }
}

fn validate_entry(idx: usize, entry: &Entry) -> Result<(), KinographError> {
    if entry.id.is_empty() {
        return Err(KinographError::InvalidEntry {
            idx,
            reason: "id is empty".into(),
        });
    }
    if let Some(pin) = entry.pin_opt() {
        Hash::from_str(pin).map_err(|e| KinographError::InvalidEntry {
            idx,
            reason: format!("pin is not a valid 64-hex hash: {e}"),
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::init::init;
    use crate::kino::{store_kino, StoreKinoParams};
    use crate::paths::kinora_root;
    use std::collections::BTreeMap;
    use tempfile::TempDir;

    fn repo() -> (TempDir, std::path::PathBuf) {
        let tmp = TempDir::new().unwrap();
        init(tmp.path(), "https://example.com/x.git").unwrap();
        let root = kinora_root(tmp.path());
        (tmp, root)
    }

    fn store_params(content: &[u8], name: &str) -> StoreKinoParams {
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

    #[test]
    fn parses_minimal_kinograph() {
        let id = "a".repeat(64);
        let input = format!("entries ({{id {id}}})");
        let k = Kinograph::parse_str(&input).unwrap();
        assert_eq!(k.entries.len(), 1);
        assert_eq!(k.entries[0].id, id);
        assert!(k.entries[0].name.is_empty());
    }

    #[test]
    fn parses_full_entry_with_all_fields() {
        let id = "a".repeat(64);
        let pin = "b".repeat(64);
        let input = format!(
            "entries ({{id {id}, name content-addressing, pin {pin}, note \"the atomic concept\"}})"
        );
        let k = Kinograph::parse_str(&input).unwrap();
        let e = &k.entries[0];
        assert_eq!(e.id, id);
        assert_eq!(e.name_opt(), Some("content-addressing"));
        assert_eq!(e.pin_opt(), Some(pin.as_str()));
        assert_eq!(e.note_opt(), Some("the atomic concept"));
    }

    #[test]
    fn roundtrip_preserves_entries() {
        let id = "a".repeat(64);
        let pin = "b".repeat(64);
        let k = Kinograph {
            entries: vec![
                Entry { id: id.clone(), name: "doc".into(), pin: String::new(), note: String::new() },
                Entry {
                    id: id.clone(),
                    name: String::new(),
                    pin: pin.clone(),
                    note: "see v1".into(),
                },
            ],
        };
        let s = k.to_styx().unwrap();
        let parsed = Kinograph::parse_str(&s).unwrap();
        assert_eq!(parsed, k);
    }

    #[test]
    fn empty_id_rejected() {
        let input = "entries ({id \"\"})";
        let err = Kinograph::parse_str(input).unwrap_err();
        assert!(matches!(err, KinographError::InvalidEntry { .. }));
    }

    #[test]
    fn non_hex_pin_rejected() {
        let id = "a".repeat(64);
        let input = format!("entries ({{id {id}, pin not-a-hash}})");
        let err = Kinograph::parse_str(&input).unwrap_err();
        assert!(matches!(err, KinographError::InvalidEntry { .. }));
    }

    #[test]
    fn resolve_names_rewrites_name_only_entry_to_id() {
        let (_t, root) = repo();
        let stored = store_kino(&root, store_params(b"hello", "greet")).unwrap();

        let k = Kinograph {
            entries: vec![Entry {
                id: "greet".into(),
                name: "greet".into(),
                pin: String::new(),
                note: String::new(),
            }],
        };
        let resolver = Resolver::load(&root).unwrap();
        let resolved = k.resolve_names(&resolver).unwrap();
        assert_eq!(resolved.entries[0].id, stored.event.id);
        assert_eq!(resolved.entries[0].name, "greet");
    }

    #[test]
    fn resolve_names_id_slot_only_is_treated_as_name() {
        let (_t, root) = repo();
        let stored = store_kino(&root, store_params(b"hello", "greet")).unwrap();

        // Author wrote the name in the id slot with no name hint — we
        // still look it up and fill in the hint for readability.
        let k = Kinograph {
            entries: vec![Entry::with_id("greet")],
        };
        let resolver = Resolver::load(&root).unwrap();
        let resolved = k.resolve_names(&resolver).unwrap();
        assert_eq!(resolved.entries[0].id, stored.event.id);
        assert_eq!(resolved.entries[0].name, "greet");
    }

    #[test]
    fn resolve_names_leaves_hash_ids_untouched() {
        let (_t, root) = repo();
        let stored = store_kino(&root, store_params(b"hi", "doc")).unwrap();
        let k = Kinograph {
            entries: vec![Entry {
                id: stored.event.id.clone(),
                name: "doc".into(),
                pin: String::new(),
                note: String::new(),
            }],
        };
        let resolver = Resolver::load(&root).unwrap();
        let out = k.resolve_names(&resolver).unwrap();
        assert_eq!(out.entries[0].id, stored.event.id);
    }

    #[test]
    fn resolve_names_errors_on_missing_name() {
        let (_t, root) = repo();
        store_kino(&root, store_params(b"x", "other")).unwrap();
        let k = Kinograph {
            entries: vec![Entry::with_id("nobody")],
        };
        let resolver = Resolver::load(&root).unwrap();
        let err = k.resolve_names(&resolver).unwrap_err();
        assert!(matches!(err, KinographError::NameNotFound { .. }));
    }

    #[test]
    fn resolve_names_errors_on_ambiguous_name() {
        let (_t, root) = repo();
        store_kino(&root, store_params(b"a", "same-name")).unwrap();
        store_kino(&root, store_params(b"b", "same-name")).unwrap();
        let k = Kinograph {
            entries: vec![Entry::with_id("same-name")],
        };
        let resolver = Resolver::load(&root).unwrap();
        let err = k.resolve_names(&resolver).unwrap_err();
        assert!(matches!(err, KinographError::AmbiguousName { .. }));
    }

    #[test]
    fn render_concatenates_entries_in_order() {
        let (_t, root) = repo();
        let a = store_kino(&root, store_params(b"# First\n\nAlpha body.", "first")).unwrap();
        let b = store_kino(&root, store_params(b"# Second\n\nBeta body.", "second")).unwrap();

        let k = Kinograph {
            entries: vec![Entry::with_id(a.event.id), Entry::with_id(b.event.id)],
        };
        let resolver = Resolver::load(&root).unwrap();
        let rendered = k.render(&resolver).unwrap();
        assert!(rendered.contains("Alpha body"));
        assert!(rendered.contains("Beta body"));
        let alpha_pos = rendered.find("Alpha body").unwrap();
        let beta_pos = rendered.find("Beta body").unwrap();
        assert!(alpha_pos < beta_pos);
    }

    #[test]
    fn render_emits_note_as_blockquote() {
        let (_t, root) = repo();
        let a = store_kino(&root, store_params(b"body text", "a")).unwrap();
        let k = Kinograph {
            entries: vec![Entry {
                id: a.event.id,
                name: String::new(),
                pin: String::new(),
                note: "why this matters".into(),
            }],
        };
        let resolver = Resolver::load(&root).unwrap();
        let rendered = k.render(&resolver).unwrap();
        assert!(rendered.contains("> why this matters"));
        let note_pos = rendered.find("> why this matters").unwrap();
        let body_pos = rendered.find("body text").unwrap();
        assert!(note_pos < body_pos);
    }

    #[test]
    fn render_uses_pin_for_specific_version() {
        let (_t, root) = repo();
        let birth = store_kino(&root, store_params(b"v1 body", "doc")).unwrap();
        let mut p = store_params(b"v2 body", "doc");
        p.id = Some(birth.event.id.clone());
        p.parents = vec![birth.event.hash.clone()];
        p.ts = "2026-04-18T10:00:01Z".into();
        store_kino(&root, p).unwrap();

        let k = Kinograph {
            entries: vec![Entry {
                id: birth.event.id.clone(),
                name: String::new(),
                pin: birth.event.hash.clone(),
                note: String::new(),
            }],
        };
        let resolver = Resolver::load(&root).unwrap();
        let rendered = k.render(&resolver).unwrap();
        assert!(rendered.contains("v1 body"));
        assert!(!rendered.contains("v2 body"));
    }

    #[test]
    fn render_multiline_note_blockquotes_each_line() {
        let (_t, root) = repo();
        let a = store_kino(&root, store_params(b"body", "a")).unwrap();
        let k = Kinograph {
            entries: vec![Entry {
                id: a.event.id,
                name: String::new(),
                pin: String::new(),
                note: "line one\nline two".into(),
            }],
        };
        let resolver = Resolver::load(&root).unwrap();
        let rendered = k.render(&resolver).unwrap();
        assert!(rendered.contains("> line one"));
        assert!(rendered.contains("> line two"));
    }

    #[test]
    fn entry_opt_accessors_treat_empty_as_none() {
        let e = Entry::with_id("x");
        assert_eq!(e.name_opt(), None);
        assert_eq!(e.pin_opt(), None);
        assert_eq!(e.note_opt(), None);
    }

    #[test]
    fn notes_with_styx_reserved_chars_roundtrip_via_quoting() {
        let id = "a".repeat(64);
        for note in [
            "see {here}",
            "\"quoted\"",
            "has @ symbol",
            "line one\nline two",
            "commas, yes, commas",
        ] {
            let k = Kinograph {
                entries: vec![Entry {
                    id: id.clone(),
                    name: String::new(),
                    pin: String::new(),
                    note: note.to_string(),
                }],
            };
            let s = k.to_styx().unwrap();
            let parsed = Kinograph::parse_str(&s).expect(note);
            assert_eq!(parsed, k, "roundtrip broke for note={note:?}");
        }
    }

    #[test]
    fn empty_entries_parses_and_renders() {
        let input = "entries ()";
        let k = Kinograph::parse_str(input).unwrap();
        assert!(k.entries.is_empty());

        let (_t, root) = repo();
        let resolver = Resolver::load(&root).unwrap();
        let rendered = k.render(&resolver).unwrap();
        assert!(rendered.is_empty());
    }

    #[test]
    fn render_errors_when_pin_does_not_match_any_version() {
        let (_t, root) = repo();
        let stored = store_kino(&root, store_params(b"x", "doc")).unwrap();
        let bogus = "b".repeat(64);
        let k = Kinograph {
            entries: vec![Entry {
                id: stored.event.id,
                name: String::new(),
                pin: bogus,
                note: String::new(),
            }],
        };
        let resolver = Resolver::load(&root).unwrap();
        let err = k.render(&resolver).unwrap_err();
        assert!(matches!(
            err,
            KinographError::Resolve(ResolveError::VersionNotFound { .. })
        ));
    }
}
