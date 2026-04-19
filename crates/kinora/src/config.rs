//! Repo-level config for a `.kinora/` tree. On-disk shape is styx; we
//! parse via facet_styx into a private raw struct, then validate/normalize
//! into the public `Config` type.
//!
//! Per-root policies (`Never`, `MaxAge(_)`, `KeepLastN(_)`) are declared
//! in a `roots {}` block. `inbox` is auto-provisioned with a default
//! `30d` policy when absent, per xi21 §6.

use std::collections::BTreeMap;

use facet::Facet;

/// Retention policy for a single root. Drives compaction-time GC in
/// hxmw-6; this bean only lands the declarative primitive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootPolicy {
    /// Never drop anything from this root.
    Never,
    /// Drop entries older than the named duration. The raw string (e.g.
    /// `"30d"`, `"12h"`) is kept verbatim — actual duration parsing is
    /// deferred to hxmw-6.
    MaxAge(String),
    /// Keep only the N most recent content versions per kino id.
    KeepLastN(usize),
}

/// Aggressive default for `inbox` per xi21 §6 — nudges users to triage.
pub const DEFAULT_INBOX_POLICY: &str = "30d";

impl RootPolicy {
    /// Parse a policy string from config.styx. Grammar:
    ///
    /// - `"never"` → `Never`
    /// - `"keep-last-<N>"` where N is a usize → `KeepLastN(N)`
    /// - `<digits><letters>` (e.g. `"30d"`, `"12h"`) → `MaxAge(raw)`
    /// - anything else → `None`
    pub fn from_policy_str(raw: &str) -> Option<Self> {
        // STUB (hxmw-c48l commit-1): always returns None so tests observe the
        // stubbed behaviour via assertions.
        let _ = raw;
        None
    }

    /// Inverse of `from_policy_str` — produces the canonical wire form.
    pub fn to_policy_str(&self) -> String {
        match self {
            RootPolicy::Never => "never".to_owned(),
            RootPolicy::MaxAge(s) => s.clone(),
            RootPolicy::KeepLastN(n) => format!("keep-last-{n}"),
        }
    }
}

/// Raw on-disk shape of the `roots { <name> { policy "<s>" } ... }` block.
#[derive(Facet, Debug, Clone, PartialEq, Eq)]
struct RawRootBlock {
    policy: String,
}

/// Raw on-disk config. `roots` is `Option<_>` so repos pre-dating the
/// `roots {}` block still parse.
#[derive(Facet, Debug, Clone, PartialEq)]
struct RawConfig {
    #[facet(rename = "repo-url")]
    repo_url: String,
    roots: Option<BTreeMap<String, RawRootBlock>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub repo_url: String,
    pub roots: BTreeMap<String, RootPolicy>,
}

#[derive(Debug)]
pub enum ConfigError {
    Parse(String),
    Serialize(String),
    InvalidPolicy { root: String, raw: String },
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Parse(m) => write!(f, "failed to parse config.styx: {m}"),
            ConfigError::Serialize(m) => write!(f, "failed to serialize config: {m}"),
            ConfigError::InvalidPolicy { root, raw } => write!(
                f,
                "invalid policy for root `{root}`: `{raw}` (expected \"never\", \"<N>[smhdwy]\", or \"keep-last-<N>\")"
            ),
        }
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    pub fn from_styx(input: &str) -> Result<Self, ConfigError> {
        let raw: RawConfig = facet_styx::from_str(input)
            .map_err(|e| ConfigError::Parse(e.to_string()))?;
        // STUB (hxmw-c48l commit-1): policies never parse, inbox is never
        // auto-provisioned. Commit 2 replaces with real logic.
        let _ = raw.roots;
        let roots: BTreeMap<String, RootPolicy> = BTreeMap::new();
        Ok(Self { repo_url: raw.repo_url, roots })
    }

    pub fn to_styx(&self) -> Result<String, ConfigError> {
        // STUB (hxmw-c48l commit-1): serialize only repo_url. Commit 2
        // adds the roots block emission.
        let raw = RawConfig {
            repo_url: self.repo_url.clone(),
            roots: None,
        };
        facet_styx::to_string(&raw).map_err(|e| ConfigError::Serialize(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with_inbox_default() -> Config {
        Config {
            repo_url: "https://github.com/edger-dev/kinora".into(),
            roots: BTreeMap::from([(
                "inbox".to_owned(),
                RootPolicy::MaxAge(DEFAULT_INBOX_POLICY.to_owned()),
            )]),
        }
    }

    // ------------------------------------------------------------------
    // Policy string grammar
    // ------------------------------------------------------------------

    #[test]
    fn policy_parses_never() {
        assert_eq!(RootPolicy::from_policy_str("never"), Some(RootPolicy::Never));
    }

    #[test]
    fn policy_parses_keep_last_n() {
        assert_eq!(
            RootPolicy::from_policy_str("keep-last-10"),
            Some(RootPolicy::KeepLastN(10))
        );
        assert_eq!(
            RootPolicy::from_policy_str("keep-last-1"),
            Some(RootPolicy::KeepLastN(1))
        );
    }

    #[test]
    fn policy_parses_max_age_durations() {
        assert_eq!(
            RootPolicy::from_policy_str("30d"),
            Some(RootPolicy::MaxAge("30d".into()))
        );
        assert_eq!(
            RootPolicy::from_policy_str("12h"),
            Some(RootPolicy::MaxAge("12h".into()))
        );
        assert_eq!(
            RootPolicy::from_policy_str("7d"),
            Some(RootPolicy::MaxAge("7d".into()))
        );
    }

    #[test]
    fn policy_rejects_bogus_input() {
        assert!(RootPolicy::from_policy_str("").is_none());
        assert!(RootPolicy::from_policy_str("forever").is_none());
        assert!(RootPolicy::from_policy_str("30").is_none(), "digits only");
        assert!(RootPolicy::from_policy_str("d").is_none(), "letters only");
        assert!(RootPolicy::from_policy_str("30d5h").is_none(), "compound not yet supported");
        assert!(RootPolicy::from_policy_str("keep-last-").is_none(), "no count");
        assert!(RootPolicy::from_policy_str("keep-last-x").is_none(), "non-numeric count");
    }

    #[test]
    fn policy_to_str_roundtrips_each_variant() {
        let cases = [
            RootPolicy::Never,
            RootPolicy::MaxAge("30d".into()),
            RootPolicy::KeepLastN(5),
        ];
        for p in cases {
            let s = p.to_policy_str();
            assert_eq!(RootPolicy::from_policy_str(&s), Some(p.clone()));
        }
    }

    // ------------------------------------------------------------------
    // from_styx / to_styx
    // ------------------------------------------------------------------

    #[test]
    fn roundtrip_minimal_config_injects_inbox_default() {
        let c = Config {
            repo_url: "https://github.com/edger-dev/kinora".into(),
            roots: BTreeMap::new(),
        };
        let s = c.to_styx().unwrap();
        let parsed = Config::from_styx(&s).unwrap();
        assert_eq!(parsed, cfg_with_inbox_default());
    }

    #[test]
    fn parses_inline_repo_url_with_no_roots_block_auto_provisions_inbox() {
        let c = Config::from_styx("repo-url https://github.com/edger-dev/kinora").unwrap();
        assert_eq!(c.repo_url, "https://github.com/edger-dev/kinora");
        assert_eq!(
            c.roots.get("inbox"),
            Some(&RootPolicy::MaxAge(DEFAULT_INBOX_POLICY.to_owned()))
        );
        assert_eq!(c.roots.len(), 1);
    }

    #[test]
    fn parses_multi_root_config() {
        let input = r#"
repo-url "https://example.com/x.git"
roots {
  inbox   { policy "30d" }
  rfcs    { policy "never" }
  designs { policy "keep-last-10" }
}
"#;
        let c = Config::from_styx(input).unwrap();
        assert_eq!(
            c.roots.get("inbox"),
            Some(&RootPolicy::MaxAge("30d".into()))
        );
        assert_eq!(c.roots.get("rfcs"), Some(&RootPolicy::Never));
        assert_eq!(
            c.roots.get("designs"),
            Some(&RootPolicy::KeepLastN(10))
        );
    }

    #[test]
    fn auto_provisions_inbox_when_roots_block_present_but_lacks_inbox() {
        let input = r#"
repo-url "https://example.com/x.git"
roots {
  rfcs { policy "never" }
}
"#;
        let c = Config::from_styx(input).unwrap();
        assert_eq!(c.roots.len(), 2);
        assert_eq!(c.roots.get("rfcs"), Some(&RootPolicy::Never));
        assert_eq!(
            c.roots.get("inbox"),
            Some(&RootPolicy::MaxAge(DEFAULT_INBOX_POLICY.to_owned()))
        );
    }

    #[test]
    fn explicit_inbox_policy_is_preserved_not_overridden() {
        let input = r#"
repo-url "https://example.com/x.git"
roots {
  inbox { policy "7d" }
}
"#;
        let c = Config::from_styx(input).unwrap();
        assert_eq!(
            c.roots.get("inbox"),
            Some(&RootPolicy::MaxAge("7d".into())),
            "user's explicit inbox policy must not be clobbered"
        );
    }

    #[test]
    fn invalid_policy_string_reported_with_root_and_raw() {
        let input = r#"
repo-url "https://example.com/x.git"
roots {
  rfcs { policy "bogus" }
}
"#;
        let err = Config::from_styx(input).unwrap_err();
        match err {
            ConfigError::InvalidPolicy { root, raw } => {
                assert_eq!(root, "rfcs");
                assert_eq!(raw, "bogus");
            }
            other => panic!("expected InvalidPolicy, got {other:?}"),
        }
    }

    #[test]
    fn roundtrip_multi_root_config() {
        let c = Config {
            repo_url: "https://github.com/edger-dev/kinora".into(),
            roots: BTreeMap::from([
                ("inbox".into(), RootPolicy::MaxAge("30d".into())),
                ("rfcs".into(), RootPolicy::Never),
                ("designs".into(), RootPolicy::KeepLastN(10)),
            ]),
        };
        let s = c.to_styx().unwrap();
        let parsed = Config::from_styx(&s).unwrap();
        assert_eq!(parsed, c);
    }

    #[test]
    fn serialized_multi_root_config_contains_all_names() {
        let c = Config {
            repo_url: "https://example.com/foo.git".into(),
            roots: BTreeMap::from([
                ("inbox".into(), RootPolicy::MaxAge("30d".into())),
                ("rfcs".into(), RootPolicy::Never),
            ]),
        };
        let s = c.to_styx().unwrap();
        assert!(s.contains("repo-url"), "got: {s}");
        assert!(s.contains("roots"), "got: {s}");
        assert!(s.contains("inbox"), "got: {s}");
        assert!(s.contains("rfcs"), "got: {s}");
        assert!(s.contains("30d"), "got: {s}");
        assert!(s.contains("never"), "got: {s}");
    }
}
